//! Recover complete declarations with the compiler's lexer and strict parser
//! (`pcbir.lsp.frontend`).
//!
//! Recovery is editor-only: it never produces a buildable board or silently
//! repairs source. Original token offsets survive synthetic root delimiters.

use copper_lang::syntax::{Document, DocumentKind};
use copper_lang::{CopperScriptError, Token, TokenKind, parse_tokens, tokenize_recovering};

/// A recovered parse: the document when its header parses, every token and
/// every error, in order.
pub struct Parsed {
    pub document: Option<Document>,
    pub tokens: Vec<Token>,
    pub errors: Vec<CopperScriptError>,
}

const STARTERS: [&str; 37] = [
    "component",
    "module",
    "net",
    "no_connect",
    "supply",
    "interface",
    "constraint",
    "member",
    "track",
    "via",
    "isolated",
    "port",
    "protected_region",
    "keepout",
    "zone",
    "polygon",
    "plane_return",
    "width_contract",
    "edge_opening",
    "configure",
    "power_state",
    "import",
    "use",
    "mechanical",
    "silkscreen",
    "physical_macro",
    "pin",
    "pad",
    "peripheral",
    "power_domain",
    "unit",
    "resource",
    "group",
    "mode_group",
    "pad_set",
    "route",
    "port",
];

fn symbol(token: &Token, text: &str) -> bool {
    token.kind == TokenKind::Symbol && token.text == text
}

/// `parse_recovering(text, filename)`.
pub fn parse_recovering(text: &str, filename: &str) -> Parsed {
    let (tokens, mut errors) = tokenize_recovering(text, filename);
    let eof = tokens.last().expect("the lexer always ends with end of file").clone();
    let close = Token {
        kind: TokenKind::Symbol,
        text: "}".to_owned(),
        location: eof.location.clone(),
        end_offset: eof.end_offset,
    };
    let header = &tokens[..tokens.len().min(3)];
    let wrapped = |body: &[Token]| {
        let mut all = header.to_vec();
        all.extend_from_slice(body);
        all.push(close.clone());
        all.push(eof.clone());
        parse_tokens(all)
    };
    let empty = match wrapped(&[]) {
        Ok(document) => document,
        Err(failure) => {
            errors.push(failure);
            return Parsed { document: None, tokens, errors };
        }
    };
    let (mut declarations, mut imports, mut libraries) = (Vec::new(), Vec::new(), Vec::new());
    let last = tokens.len() - 1;
    let mut i = 3;
    let mut closed = false;
    while i < last {
        if symbol(&tokens[i], "}") {
            closed = true;
            i += 1;
            break;
        }
        let start = i;
        let mut depth = 0usize;
        // A sibling starts at the indentation of the line the declaration
        // starts on; a block opened mid-line keeps its own indented items.
        let mut indent = tokens[start].location.column;
        for earlier in tokens[..start].iter().rev() {
            if earlier.location.line != tokens[start].location.line {
                break;
            }
            indent = earlier.location.column;
        }
        while i < last {
            let token = &tokens[i];
            // An unclosed declaration must not swallow subsequent siblings.
            if i > start
                && token.kind == TokenKind::Identifier
                && STARTERS.contains(&token.text.as_str())
                && token.location.line > tokens[start].location.line
                && token.location.column <= indent
            {
                break;
            }
            if token.kind == TokenKind::Symbol {
                if token.text == "{" {
                    depth += 1;
                } else if token.text == "}" {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        if empty.kind == DocumentKind::Macro
                            && start + 1 < tokens.len()
                            && tokens[start + 1].text == "="
                            && tokens[i].text == ";"
                        {
                            i += 1;
                        }
                        break;
                    }
                } else if token.text == ";" && depth == 0 {
                    i += 1;
                    break;
                }
            }
            i += 1;
        }
        if i == start {
            i += 1;
        }
        match wrapped(&tokens[start..i]) {
            Ok(chunk) => {
                declarations.extend(chunk.declarations);
                imports.extend(chunk.imports);
                libraries.extend(chunk.libraries);
            }
            Err(failure) => errors.push(failure),
        }
    }
    if !closed {
        errors.push(CopperScriptError::new("PAR001", "expected '}' before end of file", eof.location.clone()));
    } else if i < last {
        errors.push(CopperScriptError::new("PAR010", "expected end of file", tokens[i].location.clone()));
    }
    let document =
        Document { location: empty.location, kind: empty.kind, name: empty.name, libraries, imports, declarations };
    Parsed { document: Some(document), tokens, errors }
}
