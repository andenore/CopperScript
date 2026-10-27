//! A parser with the semantics of Python's `json.loads` (strict mode).

use std::fmt;

use num_bigint::BigInt;

use crate::{Object, Value};

/// Why a document is not JSON, with the byte offset where parsing stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (char {})", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    /// The message as Python's `json.JSONDecodeError` words it for `document`:
    /// `"Expecting value: line 1 column 1 (char 0)"`, counting characters.
    pub fn python_message(&self, document: &str) -> String {
        let before = &document[..self.offset.min(document.len())];
        let position = before.chars().count();
        let line = before.matches('\n').count() + 1;
        let column = match before.rfind('\n') {
            Some(index) => before[index + 1..].chars().count() + 1,
            None => position + 1,
        };
        format!("{}: line {line} column {column} (char {position})", self.message)
    }
}

/// Parse a complete JSON document.
///
/// Accepts what Python's `json.loads` accepts: `NaN`, `Infinity` and
/// `-Infinity`, integers of any size, and repeated object keys (the last value
/// wins, at the first key's position). Unlike Python, a lone UTF-16 surrogate
/// escape is refused because a Rust string cannot hold it.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser { text, bytes: text.as_bytes(), offset: 0 };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.offset != parser.bytes.len() {
        return Err(parser.error("Extra data"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        ParseError { message: message.to_owned(), offset: self.offset }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.offset += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, ParseError> {
        if self.text[self.offset..].starts_with(word) {
            self.offset += word.len();
            Ok(value)
        } else {
            Err(self.error("Expecting value"))
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"') => self.string().map(Value::Str),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'N') => self.literal("NaN", Value::Float(f64::NAN)),
            Some(b'I') => self.literal("Infinity", Value::Float(f64::INFINITY)),
            Some(b'-') if self.text[self.offset..].starts_with("-Infinity") => {
                self.offset += "-Infinity".len();
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error("Expecting value")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.offset;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
        self.offset - start
    }

    /// Python's NUMBER_RE: `(-?(?:0|[1-9]\d*))(\.\d+)?([eE][-+]?\d+)?`.
    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => {
                self.offset = start;
                return Err(self.error("Expecting value"));
            }
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') && matches!(self.bytes.get(self.offset + 1), Some(b'0'..=b'9')) {
            self.offset += 1;
            self.digits();
            is_float = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mark = self.offset;
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            if self.digits() == 0 {
                // Not an exponent after all: the number ends before the `e`.
                self.offset = mark;
            } else {
                is_float = true;
            }
        }
        let token = &self.text[start..self.offset];
        if is_float {
            let value: f64 = token.parse().map_err(|_| self.error("Invalid number"))?;
            Ok(Value::Float(value))
        } else {
            let value: BigInt = token.parse().map_err(|_| self.error("Invalid number"))?;
            Ok(Value::Int(value))
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self.text.get(self.offset..self.offset + 4).ok_or_else(|| self.error("Invalid \\uXXXX escape"))?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(self.error("Invalid \\uXXXX escape"));
        }
        self.offset += 4;
        Ok(u32::from_str_radix(digits, 16).expect("four hex digits"))
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.offset += 1; // opening quote
        let mut out = String::new();
        loop {
            let run_start = self.offset;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.offset += 1;
            }
            out.push_str(&self.text[run_start..self.offset]);
            match self.peek() {
                None => return Err(self.error("Unterminated string starting at")),
                Some(b'"') => {
                    self.offset += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.offset += 1;
                    let escape = self.peek().ok_or_else(|| self.error("Unterminated string starting at"))?;
                    self.offset += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            let code =
                                if (0xd800..0xdc00).contains(&unit) && self.text[self.offset..].starts_with("\\u") {
                                    let mark = self.offset;
                                    self.offset += 2;
                                    let low = self.hex4()?;
                                    if (0xdc00..0xe000).contains(&low) {
                                        0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00)
                                    } else {
                                        self.offset = mark;
                                        unit
                                    }
                                } else {
                                    unit
                                };
                            let character = char::from_u32(code).ok_or_else(|| self.error("Lone surrogate escape"))?;
                            out.push(character);
                        }
                        _ => {
                            self.offset -= 1;
                            return Err(self.error("Invalid \\escape"));
                        }
                    }
                }
                Some(_) => return Err(self.error("Invalid control character at")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, ParseError> {
        self.offset += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("Expecting ',' delimiter")),
            }
        }
    }

    fn object(&mut self) -> Result<Value, ParseError> {
        self.offset += 1;
        let mut object = Object::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(Value::Object(object));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("Expecting property name enclosed in double quotes"));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.error("Expecting ':' delimiter"));
            }
            self.offset += 1;
            self.skip_whitespace();
            let value = self.value()?;
            object.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(Value::Object(object));
                }
                _ => return Err(self.error("Expecting ',' delimiter")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_and_python_duplicate_key_semantics() {
        let value = parse(r#"{"b": 1, "a": 2, "b": 3}"#).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.keys().collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(object["b"], Value::from(3i64));
    }

    #[test]
    fn numbers_split_into_ints_and_floats_like_python() {
        assert_eq!(
            parse("123456789012345678901234567890").unwrap(),
            Value::Int("123456789012345678901234567890".parse().unwrap())
        );
        assert_eq!(parse("1.5").unwrap(), Value::Float(1.5));
        assert_eq!(parse("1e3").unwrap(), Value::Float(1000.0));
        assert_eq!(parse("-0").unwrap(), Value::Int(BigInt::from(0)));
        assert!(parse("01").is_err());
        assert!(parse("1.").is_err());
        assert!(matches!(parse("NaN").unwrap(), Value::Float(x) if x.is_nan()));
        assert_eq!(parse("-Infinity").unwrap(), Value::Float(f64::NEG_INFINITY));
    }

    #[test]
    fn strings_decode_escapes_and_surrogate_pairs() {
        assert_eq!(parse(r#""a\nb\u00e9\ud83d\ude00\/""#).unwrap(), Value::from("a\nb\u{e9}\u{1f600}/"));
        assert!(parse("\"a\u{1}b\"").is_err());
        assert!(parse(r#""\ud83d""#).is_err());
    }
}
