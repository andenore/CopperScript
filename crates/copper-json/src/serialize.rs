//! A serializer with the output of Python's `json.dumps`.

use std::fmt::Write;

use crate::Value;
use crate::float::python_float_repr;

/// Options of Python's `json.dumps`; [`Dumps::default`] matches its defaults
/// (`ensure_ascii=True`, separators `", "` and `": "`, no sorting, no indent).
#[derive(Clone, Debug)]
pub struct Dumps {
    sort_keys: bool,
    ensure_ascii: bool,
    indent: Option<String>,
    item_separator: String,
    key_separator: String,
    explicit_separators: bool,
}

impl Default for Dumps {
    fn default() -> Self {
        Dumps {
            sort_keys: false,
            ensure_ascii: true,
            indent: None,
            item_separator: ", ".to_owned(),
            key_separator: ": ".to_owned(),
            explicit_separators: false,
        }
    }
}

impl Dumps {
    /// Python's defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// `separators=(",", ":")`, the form state files and digests use.
    pub fn compact() -> Self {
        Self::new().separators(",", ":")
    }

    pub fn sort_keys(mut self, sort_keys: bool) -> Self {
        self.sort_keys = sort_keys;
        self
    }

    pub fn ensure_ascii(mut self, ensure_ascii: bool) -> Self {
        self.ensure_ascii = ensure_ascii;
        self
    }

    /// `indent=N`. As in Python, an indent without explicit separators uses
    /// `","` between items so lines carry no trailing space.
    pub fn indent(mut self, spaces: usize) -> Self {
        self.indent = Some(" ".repeat(spaces));
        self
    }

    pub fn separators(mut self, item: &str, key: &str) -> Self {
        self.item_separator = item.to_owned();
        self.key_separator = key.to_owned();
        self.explicit_separators = true;
        self
    }

    pub fn dumps(&self, value: &Value) -> String {
        let item_separator =
            if self.indent.is_some() && !self.explicit_separators { "," } else { &self.item_separator };
        let mut out = String::new();
        self.write(&mut out, value, item_separator, 0);
        out
    }

    fn newline(&self, out: &mut String, level: usize) {
        if let Some(indent) = &self.indent {
            out.push('\n');
            for _ in 0..level {
                out.push_str(indent);
            }
        }
    }

    fn write(&self, out: &mut String, value: &Value, item_separator: &str, level: usize) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Int(number) => write!(out, "{number}").expect("writing to a String"),
            Value::Float(number) => out.push_str(&json_float(*number)),
            Value::Str(text) => encode_str_into(out, text, self.ensure_ascii),
            Value::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                self.newline(out, level + 1);
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(item_separator);
                        self.newline(out, level + 1);
                    }
                    self.write(out, item, item_separator, level + 1);
                }
                self.newline(out, level);
                out.push(']');
            }
            Value::Object(object) => {
                if object.is_empty() {
                    out.push_str("{}");
                    return;
                }
                let mut entries: Vec<(&String, &Value)> = object.iter().collect();
                if self.sort_keys {
                    entries.sort_by(|a, b| a.0.cmp(b.0));
                }
                out.push('{');
                self.newline(out, level + 1);
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        out.push_str(item_separator);
                        self.newline(out, level + 1);
                    }
                    encode_str_into(out, key, self.ensure_ascii);
                    out.push_str(&self.key_separator);
                    self.write(out, item, item_separator, level + 1);
                }
                self.newline(out, level);
                out.push('}');
            }
        }
    }
}

/// Python's float encoding in JSON: `repr`, with `NaN`/`Infinity` spelled out.
fn json_float(number: f64) -> String {
    if number.is_nan() {
        "NaN".to_owned()
    } else if number == f64::INFINITY {
        "Infinity".to_owned()
    } else if number == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        python_float_repr(number)
    }
}

/// A JSON string literal as `json.dumps(text, ensure_ascii=...)` writes it.
pub fn encode_str(text: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    encode_str_into(&mut out, text, ensure_ascii);
    out
}

fn encode_str_into(out: &mut String, text: &str, ensure_ascii: bool) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // ensure_ascii keeps only printable ASCII (space through tilde).
            ' '..='~' => out.push(character),
            _ if character < ' ' => write!(out, "\\u{:04x}", character as u32).expect("writing to a String"),
            _ if !ensure_ascii => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    write!(out, "\\u{unit:04x}").expect("writing to a String");
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn python_separator_and_indent_rules() {
        let value = parse(r#"{"b": [1, 2.0, {}], "a": "x"}"#).unwrap();
        assert_eq!(Dumps::new().dumps(&value), r#"{"b": [1, 2.0, {}], "a": "x"}"#);
        assert_eq!(Dumps::compact().sort_keys(true).dumps(&value), r#"{"a":"x","b":[1,2.0,{}]}"#);
        assert_eq!(
            Dumps::new().indent(2).dumps(&value),
            "{\n  \"b\": [\n    1,\n    2.0,\n    {}\n  ],\n  \"a\": \"x\"\n}"
        );
    }

    #[test]
    fn ascii_escaping_matches_python() {
        assert_eq!(
            encode_str("a\"\\\u{7f}\u{e9}\u{1f600}\u{1}", true),
            "\"a\\\"\\\\\\u007f\\u00e9\\ud83d\\ude00\\u0001\""
        );
        assert_eq!(encode_str("\u{7f}\u{e9}\u{1}", false), "\"\u{7f}\u{e9}\\u0001\"");
    }
}
