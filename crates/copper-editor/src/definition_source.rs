//! `pcbir.editor.definitions`: the source inventory and token patches of
//! definition documents (macro and board_profile).

use copper_json::{Dumps, Object, Value};
use copper_lang::syntax::{Declaration, Document, DocumentKind, MacroItemDecl, MechanicalItem};
use copper_lang::{Token, TokenKind, parse, tokenize};
use indexmap::IndexMap;

use crate::source::{SourcePatch, SourceSnapshot, SourceSpan, TextEdit, assignments, checked};
use crate::{Error, Result, object};

fn source_edit<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

/// `json.dumps(text)`.
pub fn dumps(text: &str) -> String {
    Dumps::new().dumps(&Value::from(text))
}

fn span(token: &Token) -> SourceSpan {
    SourceSpan { start: token.location.offset, end: token.end_offset }
}

fn opens(token: &Token) -> bool {
    token.kind == TokenKind::Symbol && matches!(token.text.as_str(), "{" | "[" | "(")
}

fn closes(token: &Token) -> bool {
    token.kind == TokenKind::Symbol && matches!(token.text.as_str(), "}" | "]" | ")")
}

/// `_node_tokens`: the tokens of the declaration starting at `offset`.
pub fn node_tokens(tokens: &[Token], offset: usize) -> Result<&[Token]> {
    let start = tokens.iter().position(|t| t.location.offset == offset).expect("declaration token");
    let assignment = tokens.get(start + 1).is_some_and(|t| t.text == "=");
    let mut depth = 0i64;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        if opens(token) {
            depth += 1;
        } else if closes(token) {
            depth -= 1;
            if depth == 0 && token.text == "}" && !assignment {
                return Ok(&tokens[start..=i]);
            }
        } else if token.kind == TokenKind::Symbol && token.text == ";" && depth == 0 {
            return Ok(&tokens[start..=i]);
        }
    }
    source_edit("incomplete definition declaration")
}

/// `_properties`: the node's direct assignments, by name, as token runs.
pub fn properties(tokens: &[Token]) -> IndexMap<String, &[Token]> {
    let mut i = tokens.iter().position(|t| t.kind == TokenKind::Symbol && t.text == "{").expect("block") + 1;
    let mut result = IndexMap::new();
    let last = tokens.len() - 1;
    while i < last {
        let begin = i;
        let mut depth = 0i64;
        let assignment = tokens.get(i + 1).is_some_and(|t| t.text == "=");
        while i < last {
            let token = &tokens[i];
            if opens(token) {
                depth += 1;
            } else if closes(token) {
                depth -= 1;
                if depth == 0 && token.text == "}" && !assignment {
                    i += 1;
                    break;
                }
            } else if token.kind == TokenKind::Symbol && token.text == ";" && depth == 0 {
                if assignment {
                    result.insert(tokens[begin].text.clone(), &tokens[begin..=i]);
                }
                i += 1;
                break;
            }
            i += 1;
        }
    }
    result
}

/// `text[run[2].start:run[-1].start].strip()`: an assignment's value text.
fn value_text(snapshot: &SourceSnapshot, run: &[Token]) -> String {
    snapshot.slice(run[2].location.offset, run[run.len() - 1].location.offset).trim().to_owned()
}

fn category(document: &Document) -> &'static str {
    if document.kind == DocumentKind::Macro { "macro" } else { "profile" }
}

fn feature(category: &str, kind: &str, name: &str, shape: &str, parent: &str, parameters: Object) -> Value {
    object(vec![
        ("category", Value::from(category)),
        ("kind", Value::from(kind)),
        ("name", Value::from(name)),
        ("shape", Value::from(shape)),
        ("parent", Value::from(parent)),
        ("parameters", Value::Object(parameters)),
    ])
}

/// A macro item or a profile feature: its identity, shape and children.
struct Item<'a> {
    offset: usize,
    kind: &'a str,
    name: &'a str,
    shape: &'a str,
    children: &'a [MacroItemDecl],
}

fn item(declaration: &Declaration) -> Option<Item<'_>> {
    match declaration {
        Declaration::MacroItem(m) => {
            Some(Item { offset: m.location.offset, kind: &m.kind, name: &m.name, shape: "", children: &m.children })
        }
        Declaration::MechanicalItem(MechanicalItem::Feature(f)) => {
            Some(Item { offset: f.location.offset, kind: &f.kind, name: &f.name, shape: &f.shape, children: &[] })
        }
        _ => None,
    }
}

fn macro_child(child: &MacroItemDecl) -> Item<'_> {
    Item { offset: child.location.offset, kind: &child.kind, name: &child.name, shape: "", children: &child.children }
}

/// `definition_features`: the editable inventory of a definition document.
pub fn definition_features(snapshot: &SourceSnapshot) -> Result<Vec<Value>> {
    let document = parse(&snapshot.text, &snapshot.filename)?;
    let tokens = tokenize(&snapshot.text, &snapshot.filename)?;
    let category = category(&document);
    let mut features = Vec::new();
    let mut root = Object::new();
    let add = |features: &mut Vec<Value>, item: &Item, parent: &str| -> Result<()> {
        let run = node_tokens(&tokens, item.offset)?;
        let parameters: Object =
            properties(run).iter().map(|(k, v)| (k.clone(), Value::from(value_text(snapshot, v)))).collect();
        features.push(feature(category, item.kind, item.name, item.shape, parent, parameters));
        Ok(())
    };
    for node in &document.declarations {
        match node {
            Declaration::MacroPad(pad) => {
                let mut parameters = Object::new();
                parameters.insert("member".to_owned(), Value::from(dumps(&pad.reference.member)));
                parameters.insert("pad".to_owned(), Value::from(dumps(&pad.reference.pad)));
                if let Some(net) = &pad.net {
                    parameters.insert("net".to_owned(), Value::from(dumps(net)));
                }
                let kind = if pad.net.is_some() { "pad" } else { "isolated" };
                let name = format!("{}.{}", pad.reference.member, pad.reference.pad);
                features.push(feature(category, kind, &name, "", "", parameters));
            }
            Declaration::MacroProperty(property) => {
                let run = node_tokens(&tokens, property.location.offset)?;
                root.insert(property.name.clone(), Value::from(value_text(snapshot, run)));
            }
            Declaration::MacroItem(m) if m.kind == "net" => {
                features.push(feature(category, "net", &m.name, "", "", Object::new()));
            }
            other => {
                let Some(item) = item(other) else { continue };
                add(&mut features, &item, "")?;
                for child in item.children {
                    add(&mut features, &macro_child(child), item.name)?;
                }
            }
        }
    }
    if !root.is_empty() {
        features.insert(0, feature(category, "properties", "", "", "", root));
    }
    Ok(features)
}

fn erase(edits: &mut Vec<TextEdit>, run: &[Token]) {
    edits.extend(run.iter().map(|t| TextEdit::new(span(t), "")));
}

fn replace_value(edits: &mut Vec<TextEdit>, run: &[Token], value: &str) {
    let values = &run[2..run.len() - 1];
    edits.extend(values.iter().enumerate().map(|(i, t)| TextEdit::new(span(t), if i == 0 { value } else { "" })));
}

/// `definition_patch`: one bounded edit of a definition document. `fields`
/// are the keyword arguments (`category`, `kind`, `name`, `parameters`,
/// `shape`, `parent`, `remove`).
pub fn definition_patch(snapshot: &SourceSnapshot, fields: &Object) -> Result<SourcePatch> {
    let document = parse(&snapshot.text, &snapshot.filename)?;
    let expected = category(&document);
    if fields.get("category").and_then(Value::as_str) != Some(expected) {
        return source_edit("feature category does not match document");
    }
    let parameters: Vec<(String, String)> = match fields.get("parameters") {
        Some(Value::Object(map))
            if map.len() <= 40 && map.values().all(|v| matches!(v, Value::Str(s) if s.chars().count() <= 65536)) =>
        {
            map.iter().map(|(k, v)| (k.clone(), v.as_str().expect("str").to_owned())).collect()
        }
        _ => return source_edit("feature requires bounded source literal parameters"),
    };
    let empty = Value::from("");
    let text = |key: &str, fallback: &Value| fields.get(key).unwrap_or(fallback).as_str().map(str::to_owned);
    let remove = fields.get("remove").cloned().unwrap_or(Value::Bool(false));
    let (Some(kind), Some(name), Some(shape), Some(parent), Value::Bool(remove)) =
        (text("kind", &Value::Null), text("name", &Value::Null), text("shape", &empty), text("parent", &empty), remove)
    else {
        return source_edit("invalid feature identity");
    };
    let is_macro = document.kind == DocumentKind::Macro;
    let tokens = tokenize(&snapshot.text, &snapshot.filename)?;
    let newline = snapshot.newline();
    let end = tokens[tokens.len() - 2].location.offset;
    let has = |key: &str| parameters.iter().any(|(k, _)| k == key);
    let get = |key: &str| parameters.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("");
    let mut edits = Vec::new();
    if is_macro && matches!(kind.as_str(), "pad" | "isolated" | "net") {
        if !parent.is_empty() || !shape.is_empty() {
            return source_edit("pad/net declarations have no parent or shape");
        }
        let (literal, matches): (String, Vec<usize>) = if kind == "net" {
            if !parameters.is_empty() {
                return source_edit("net declaration has no properties");
            }
            let matches = document
                .declarations
                .iter()
                .filter_map(|d| match d {
                    Declaration::MacroItem(m) if m.kind == "net" && m.name == name => Some(m.location.offset),
                    _ => None,
                })
                .collect();
            (format!("net {};", dumps(&name)), matches)
        } else {
            let required: &[&str] = if kind == "pad" { &["member", "pad", "net"] } else { &["member", "pad"] };
            if parameters.len() != required.len() || !required.iter().all(|k| has(k)) {
                return source_edit("pad assignment requires member, pad and net; isolated requires member and pad");
            }
            let net = if kind == "pad" { format!(" = {}", get("net")) } else { String::new() };
            let matches = document
                .declarations
                .iter()
                .filter_map(|d| match d {
                    Declaration::MacroPad(p) if format!("{}.{}", p.reference.member, p.reference.pad) == name => {
                        Some(p.location.offset)
                    }
                    _ => None,
                })
                .collect();
            (format!("{kind} {}.{}{net};", get("member"), get("pad")), matches)
        };
        let scratch = parse(&format!("macro Candidate {{ {literal} }}"), "<memory>")?;
        if scratch.declarations.len() != 1 {
            return source_edit("one declaration required");
        }
        if matches.len() > 1 {
            return source_edit("ambiguous declaration");
        }
        if let Some(offset) = matches.first() {
            erase(&mut edits, node_tokens(&tokens, *offset)?);
            if !remove {
                edits[0].replacement = literal;
            }
        } else if remove {
            return source_edit("declaration is not owned by this definition");
        } else {
            edits.push(TextEdit::new(SourceSpan { start: end, end }, format!("{newline}    {literal}{newline}")));
        }
    } else if is_macro && kind == "properties" {
        let known = parameters.iter().all(|(k, _)| copper_lang::macros::ROOT_PROPERTIES.contains(&k.as_str()));
        if !name.is_empty() || !parent.is_empty() || remove || !known {
            return source_edit("invalid macro root properties");
        }
        let scratch = parse(&format!("macro Candidate {{ {} }}", assignments(&parameters)), "<memory>")?;
        let literal = scratch.declarations.len() == parameters.len()
            && scratch.declarations.iter().all(|d| matches!(d, Declaration::MacroProperty(p) if has(&p.name)));
        if !literal {
            return source_edit("root properties must contain only literal assignments");
        }
        let mut remaining = parameters.clone();
        for node in &document.declarations {
            let Declaration::MacroProperty(property) = node else { continue };
            let run = node_tokens(&tokens, property.location.offset)?;
            match remaining.iter().position(|(k, _)| *k == property.name) {
                Some(index) => {
                    let (_, value) = remaining.remove(index);
                    replace_value(&mut edits, run, &value);
                }
                None => erase(&mut edits, run),
            }
        }
        if !remaining.is_empty() {
            let lines: String = remaining.iter().map(|(k, v)| format!("    {k} = {v};{newline}")).collect();
            edits.push(TextEdit::new(SourceSpan { start: end, end }, format!("{newline}{lines}")));
        }
    } else {
        let top: Vec<Item> = document.declarations.iter().filter_map(item).collect();
        let mut owner: Option<&Item> = None;
        let mut candidates: Vec<Item> = Vec::new();
        if !parent.is_empty() {
            let parents: Vec<&Item> = top
                .iter()
                .filter(|i| i.kind == "plane_return" && i.name == parent && i.shape.is_empty() && is_macro)
                .collect();
            if parents.len() != 1 || kind != "contact" {
                return source_edit("contact requires one existing plane_return parent");
            }
            owner = Some(parents[0]);
            candidates.extend(parents[0].children.iter().map(macro_child));
        }
        let declarations = if owner.is_some() { &candidates } else { &top };
        let matches: Vec<&Item> = declarations.iter().filter(|i| i.kind == kind && i.name == name).collect();
        if matches.len() > 1 {
            return source_edit("ambiguous definition ownership");
        }
        let shown = if name.is_empty() {
            String::new()
        } else if is_macro {
            dumps(&name)
        } else {
            name.clone()
        };
        let header: Vec<&str> =
            [kind.as_str(), shown.as_str(), shape.as_str()].into_iter().filter(|v| !v.is_empty()).collect();
        let literal = format!("{} {{ {} }}", header.join(" "), assignments(&parameters));
        let scratch = if parent.is_empty() {
            parse(&format!("{} Candidate {{ {literal} }}", document.kind.as_str()), "<memory>")?
        } else {
            parse(&format!("macro Candidate {{ plane_return P {{ {literal} }} }}"), "<memory>")?
        };
        let candidate: Option<Item> = if parent.is_empty() {
            scratch.declarations.first().and_then(item)
        } else {
            match scratch.declarations.first() {
                Some(Declaration::MacroItem(m)) => match m.children.first() {
                    Some(child) => Some(macro_child(child)),
                    None => return Err(Error::Uncaught("IndexError: tuple index out of range".to_owned())),
                },
                _ => None,
            }
        };
        let child_count = match scratch.declarations.first() {
            Some(Declaration::MacroItem(m)) => m.children.len(),
            _ => 0,
        };
        let candidate_parameters: Option<Vec<String>> = if parent.is_empty() {
            match scratch.declarations.first() {
                Some(Declaration::MacroItem(m)) => Some(m.parameters.keys().cloned().collect()),
                Some(Declaration::MechanicalItem(MechanicalItem::Feature(f))) => {
                    Some(f.parameters.keys().cloned().collect())
                }
                _ => None,
            }
        } else {
            match scratch.declarations.first() {
                Some(Declaration::MacroItem(m)) => m.children.first().map(|c| c.parameters.keys().cloned().collect()),
                _ => None,
            }
        };
        let valid = scratch.declarations.len() == 1
            && (parent.is_empty() || child_count == 1)
            && candidate.as_ref().is_some_and(|c| c.kind == kind && c.name == name && c.children.is_empty())
            && candidate_parameters
                .is_some_and(|keys| keys.len() == parameters.len() && parameters.iter().all(|(k, _)| keys.contains(k)));
        if !valid {
            return source_edit("feature must contain exactly one literal declaration");
        }
        if let Some(node) = matches.first() {
            let run = node_tokens(&tokens, node.offset)?;
            if remove {
                erase(&mut edits, run);
            } else if shape != node.shape {
                return source_edit("remove and recreate a feature to change its shape");
            } else {
                let mut remaining = parameters.clone();
                for (key, values) in properties(run) {
                    match remaining.iter().position(|(k, _)| *k == key) {
                        Some(index) => {
                            let (_, value) = remaining.remove(index);
                            replace_value(&mut edits, values, &value);
                        }
                        None => erase(&mut edits, values),
                    }
                }
                if !remaining.is_empty() {
                    let offset = run[run.len() - 1].location.offset;
                    let at = SourceSpan { start: offset, end: offset };
                    edits.push(TextEdit::new(at, format!(" {} ", assignments(&remaining))));
                }
            }
        } else if remove {
            return source_edit("feature is not owned by this definition");
        } else {
            let offset = match owner {
                Some(owner) => {
                    let run = node_tokens(&tokens, owner.offset)?;
                    run[run.len() - 1].location.offset
                }
                None => end,
            };
            edits.push(TextEdit::new(
                SourceSpan { start: offset, end: offset },
                format!("{newline}    {literal}{newline}"),
            ));
        }
    }
    checked(snapshot, SourcePatch { revision: snapshot.revision(), edits })
}
