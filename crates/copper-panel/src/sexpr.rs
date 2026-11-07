//! KiCad s-expressions that keep every atom exactly as written (quoted
//! strings keep their quotes and escapes), so a composed panel reprints the
//! items it does not change faithfully.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// An atom as written: a bare word, a number, or a quoted string with its quotes.
    Atom(String),
    List(Vec<Node>),
}

impl Node {
    pub fn list(items: Vec<Node>) -> Node {
        Node::List(items)
    }

    pub fn atom(text: impl Into<String>) -> Node {
        Node::Atom(text.into())
    }

    /// A quoted string atom.
    pub fn string(text: &str) -> Node {
        Node::Atom(quote(text))
    }

    /// `(tag value...)` of atoms.
    pub fn tagged(tag: &str, values: &[&str]) -> Node {
        let mut items = vec![Node::atom(tag)];
        items.extend(values.iter().map(|v| Node::atom(*v)));
        Node::List(items)
    }

    pub fn items(&self) -> &[Node] {
        match self {
            Node::List(items) => items,
            Node::Atom(_) => &[],
        }
    }

    pub fn items_mut(&mut self) -> Option<&mut Vec<Node>> {
        match self {
            Node::List(items) => Some(items),
            Node::Atom(_) => None,
        }
    }

    /// The list's leading bare word.
    pub fn tag(&self) -> Option<&str> {
        match self.items().first() {
            Some(Node::Atom(text)) if !text.starts_with('"') => Some(text),
            _ => None,
        }
    }

    pub fn atom_text(&self) -> Option<&str> {
        match self {
            Node::Atom(text) => Some(text),
            Node::List(_) => None,
        }
    }

    /// The `index`-th item as an atom (index 0 is the tag).
    pub fn value(&self, index: usize) -> Option<&str> {
        self.items().get(index).and_then(Node::atom_text)
    }

    /// The `index`-th item as an unquoted string.
    pub fn string_value(&self, index: usize) -> Option<String> {
        self.value(index).map(unquote)
    }

    pub fn children<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.items().iter().filter(move |n| n.tag() == Some(tag))
    }

    pub fn child(&self, tag: &str) -> Option<&Node> {
        self.items().iter().find(|n| n.tag() == Some(tag))
    }

    pub fn child_mut(&mut self, tag: &str) -> Option<&mut Node> {
        self.items_mut()?.iter_mut().find(|n| n.tag() == Some(tag))
    }

    /// Visit this node and every list below it, depth first.
    pub fn walk_mut(&mut self, visit: &mut dyn FnMut(&mut Node)) {
        visit(self);
        if let Node::List(items) = self {
            for item in items {
                item.walk_mut(visit);
            }
        }
    }
}

pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else { return text.to_owned() };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse one root list.
pub fn parse(text: &str, source: &str) -> Result<Node, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut stack: Vec<Vec<Node>> = Vec::new();
    let mut root: Option<Node> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            if root.is_some() {
                return Err(format!("{source}: text after the root list"));
            }
            stack.push(Vec::new());
            i += 1;
        } else if c == ')' {
            let items = stack.pop().ok_or_else(|| format!("{source}: unbalanced ')'"))?;
            match stack.last_mut() {
                Some(parent) => parent.push(Node::List(items)),
                None => root = Some(Node::List(items)),
            }
            i += 1;
        } else {
            let start = i;
            if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(format!("{source}: unterminated string"));
                }
                i += 1;
            } else {
                while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '(' && chars[i] != ')' {
                    i += 1;
                }
            }
            let atom: String = chars[start..i].iter().collect();
            stack.last_mut().ok_or_else(|| format!("{source}: atom outside a list"))?.push(Node::Atom(atom));
        }
    }
    if !stack.is_empty() {
        return Err(format!("{source}: unclosed '('"));
    }
    root.ok_or_else(|| format!("{source}: no root list"))
}

/// Print KiCad style: a list of atoms on one line, lists with sublists one
/// item per line, indented by two spaces.
pub fn print(node: &Node) -> String {
    let mut out = String::new();
    write_node(node, 0, &mut out);
    out.push('\n');
    out
}

fn write_node(node: &Node, depth: usize, out: &mut String) {
    match node {
        Node::Atom(text) => out.push_str(text),
        Node::List(items) => {
            let nested = items
                .iter()
                .any(|n| matches!(n, Node::List(inner) if inner.iter().any(|m| matches!(m, Node::List(_)))));
            let short = items.iter().all(|n| matches!(n, Node::Atom(_)))
                || (!nested && items.iter().filter(|n| matches!(n, Node::List(_))).count() <= 4);
            out.push('(');
            if short {
                for (k, item) in items.iter().enumerate() {
                    if k > 0 {
                        out.push(' ');
                    }
                    write_node(item, depth + 1, out);
                }
            } else {
                let mut first_list = true;
                for (k, item) in items.iter().enumerate() {
                    match item {
                        Node::Atom(text) if first_list => {
                            if k > 0 {
                                out.push(' ');
                            }
                            out.push_str(text);
                        }
                        _ => {
                            first_list = false;
                            out.push('\n');
                            let _ = write!(out, "{}", "  ".repeat(depth + 1));
                            write_node(item, depth + 1, out);
                        }
                    }
                }
                out.push('\n');
                let _ = write!(out, "{}", "  ".repeat(depth));
            }
            out.push(')');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_round_trip_as_written() {
        let text = r#"(kicad_pcb (version 20240108) (net 1 "A \"q\" b") (footprint "lib:x" (at 1.5 -2 90) (pad "1" smd rect (at 0 0) (size 1 1))))"#;
        let node = parse(text, "t").expect("parsed");
        let again = parse(&print(&node), "t").expect("reparsed");
        assert_eq!(node, again);
        assert_eq!(node.child("net").and_then(|n| n.string_value(2)).as_deref(), Some("A \"q\" b"));
        assert_eq!(node.child("footprint").and_then(|f| f.child("at")).and_then(|a| a.value(3)), Some("90"));
    }

    #[test]
    fn malformed_text_is_refused() {
        assert!(parse("(a (b)", "t").is_err());
        assert!(parse("(a) (b)", "t").is_err());
        assert!(parse("(a \"unterminated)", "t").is_err());
    }
}
