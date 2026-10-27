//! LSP UTF-16 coordinates and file URIs (`pcbir.lsp.text`).
//!
//! Sources keep their exact characters, CRLF included; offsets count code
//! points like Python strings, and LSP positions count UTF-16 units.

use std::path::{Path, PathBuf};

use copper_json::{Object, Value, python_isspace};
use copper_lang::paths;
use num_bigint::BigInt;

use crate::pyerr::{PyError, PyResult, exact_int, get, item, iterate, type_name};

/// An object value from `(key, value)` pairs, in order.
pub fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

pub fn int(value: usize) -> Value {
    Value::Int(BigInt::from(value))
}

/// A document's text with its line starts.
#[derive(Clone, Debug)]
pub struct Text {
    pub source: String,
    pub chars: Vec<char>,
    pub starts: Vec<usize>,
}

impl Text {
    pub fn new(source: &str) -> Text {
        let chars: Vec<char> = source.chars().collect();
        let mut starts = vec![0];
        starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
        Text { source: source.to_owned(), chars, starts }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// `source[start:end]`, clamped like a Python slice.
    pub fn slice(&self, start: usize, end: usize) -> String {
        let end = end.min(self.chars.len());
        if start >= end { String::new() } else { self.chars[start..end].iter().collect() }
    }

    pub fn position(&self, offset: usize) -> Value {
        let offset = offset.min(self.chars.len());
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        let character: usize = self.chars[self.starts[line]..offset].iter().map(|c| c.len_utf16()).sum();
        object([("line", int(line)), ("character", int(character))])
    }

    pub fn offset(&self, position: &Value) -> PyResult<usize> {
        let line = item(position, "line")?;
        let character = item(position, "character")?;
        let invalid = || PyError::Value("invalid text position".to_owned());
        let (Some(line), Some(character)) = (exact_int(line), exact_int(character)) else {
            return Err(invalid());
        };
        let line = usize::try_from(line).ok().filter(|line| *line < self.starts.len()).ok_or_else(invalid)?;
        if character.sign() == num_bigint::Sign::Minus {
            return Err(invalid());
        }
        let character = usize::try_from(character).unwrap_or(usize::MAX);
        let start = self.starts[line];
        let end = self.starts.get(line + 1).copied().unwrap_or(self.chars.len());
        let mut content = &self.chars[start..end];
        while let [rest @ .., '\r' | '\n'] = content {
            content = rest;
        }
        let mut units = 0;
        for (index, c) in content.iter().enumerate() {
            if units == character {
                return Ok(start + index);
            }
            units += c.len_utf16();
            if units > character {
                return Err(PyError::Value("position splits a UTF-16 surrogate pair".to_owned()));
            }
        }
        if units == character {
            return Ok(start + content.len());
        }
        Err(PyError::Value("position is past end of line".to_owned()))
    }

    pub fn range(&self, start: usize, end: usize) -> Value {
        object([("start", self.position(start)), ("end", self.position(end))])
    }

    /// `Text.change`: apply `contentChanges` in order.
    pub fn change(&self, changes: &Value) -> PyResult<String> {
        let mut source = self.source.clone();
        for change in iterate(changes)? {
            let Some(Value::Str(text)) = get(&change, "text")? else {
                return Err(PyError::Value("change text must be a string".to_owned()));
            };
            if get(&change, "range")?.is_none() {
                source = text.clone();
                continue;
            }
            let current = Text::new(&source);
            let range = item(&change, "range")?;
            let start = current.offset(item(range, "start")?)?;
            let end = current.offset(item(range, "end")?)?;
            if end < start {
                return Err(PyError::Value("reversed change range".to_owned()));
            }
            source = format!("{}{text}{}", current.slice(0, start), current.slice(end, current.len()));
        }
        Ok(source)
    }
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(python_isspace)
}

/// The parts of `urllib.parse.urlsplit`.
struct Split {
    scheme: String,
    netloc: String,
    path: String,
    query: String,
    fragment: String,
}

const SCHEME_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";

fn urlsplit(url: &str) -> PyResult<Split> {
    let mut url: String =
        url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !"\t\r\n".contains(*c)).collect();
    let mut scheme = String::new();
    let mut netloc = String::new();
    let mut query = String::new();
    let mut fragment = String::new();
    if let Some(colon) = url.find(':') {
        let first = url.chars().next().unwrap_or(' ');
        if colon > 0 && first.is_ascii_alphabetic() && url[..colon].chars().all(|c| SCHEME_CHARS.contains(c)) {
            scheme = url[..colon].to_ascii_lowercase();
            url = url[colon + 1..].to_owned();
        }
    }
    if url.starts_with("//") {
        let delimiter = ['/', '?', '#'].iter().filter_map(|c| url[2..].find(*c).map(|i| i + 2)).min();
        let delimiter = delimiter.unwrap_or(url.len());
        netloc = url[2..delimiter].to_owned();
        url = url[delimiter..].to_owned();
        if netloc.contains('[') != netloc.contains(']') {
            return Err(PyError::Value("Invalid IPv6 URL".to_owned()));
        }
    }
    if let Some((rest, tail)) = url.split_once('#') {
        fragment = tail.to_owned();
        url = rest.to_owned();
    }
    if let Some((rest, tail)) = url.split_once('?') {
        query = tail.to_owned();
        url = rest.to_owned();
    }
    Ok(Split { scheme, netloc, path: url, query, fragment })
}

/// `urllib.parse.unquote` to bytes (UTF-8, `surrogateescape` on decode).
fn unquote(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("ASCII hex digits");
            out.push(u8::from_str_radix(hex, 16).expect("hex digits"));
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

/// VS Code percent-encodes the drive colon (`file:///c%3A/...`):
/// `^/([A-Za-z])(?:%3[Aa]|%7[Cc])(?=/|$)` becomes `/c:`.
fn decode_drive(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 5
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b'%'
        && matches!(&bytes[3..5], b"3A" | b"3a" | b"7C" | b"7c")
        && (bytes.len() == 5 || bytes[5] == b'/')
    {
        return format!("/{}:{}", bytes[1] as char, &path[5..]);
    }
    path.to_owned()
}

/// `url2pathname` of this platform.
#[cfg(not(windows))]
fn url2pathname(path: &str) -> PyResult<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let path = if path.starts_with("///") {
        &path[2..]
    } else if path.starts_with("//localhost/") {
        &path[11..]
    } else {
        path
    };
    Ok(PathBuf::from(std::ffi::OsString::from_vec(unquote(path))))
}

/// `nturl2path.url2pathname`.
#[cfg(windows)]
fn url2pathname(url: &str) -> PyResult<PathBuf> {
    let mut url = url;
    if url.starts_with("///") {
        url = &url[2..];
    } else if url.starts_with("//localhost/") {
        url = &url[11..];
    }
    if url.starts_with("///") {
        url = &url[1..];
    }
    let url = url.replace(':', "|");
    let decode = |text: &str| String::from_utf8_lossy(&unquote(&text.replace('/', "\\"))).into_owned();
    if !url.contains('|') {
        return Ok(PathBuf::from(decode(&url)));
    }
    let parts: Vec<&str> = url.split('|').collect();
    let letter = parts[0].chars().last().filter(|c| c.is_ascii_alphabetic());
    let (2, Some(letter)) = (parts.len(), letter) else {
        return Err(PyError::Os(format!("Bad URL: {url}")));
    };
    Ok(PathBuf::from(format!("{}:{}", letter.to_ascii_uppercase(), decode(parts[1]))))
}

/// `local_path_text(uri)` as a path; non-string URIs fail like `urlsplit`.
fn local_path(uri: &Value) -> PyResult<PathBuf> {
    let required = || PyError::Value("a local file URI is required".to_owned());
    let uri = match uri {
        Value::Str(text) => text,
        Value::Array(_) | Value::Object(_) => {
            return Err(PyError::Type(format!("unhashable type: '{}'", type_name(uri))));
        }
        other if !crate::pyerr::truthy(other) => return Err(required()),
        other => return Err(PyError::Attribute(format!("'{}' object has no attribute 'decode'", type_name(other)))),
    };
    let parsed = urlsplit(uri)?;
    if parsed.scheme != "file" || !parsed.query.is_empty() || !parsed.fragment.is_empty() {
        return Err(required());
    }
    if !parsed.netloc.is_empty() && parsed.netloc != "localhost" {
        return Err(PyError::Value("remote file hosts are unsupported".to_owned()));
    }
    url2pathname(&decode_drive(&parsed.path))
}

/// `path_from_uri`: the resolved local path of a `file:` URI.
pub fn path_from_uri(uri: &Value) -> PyResult<PathBuf> {
    let path = local_path(uri)?;
    Ok(paths::resolve(if path.as_os_str().is_empty() { Path::new(".") } else { &path }))
}

/// `urllib.parse.quote_from_bytes(data, safe="/")`.
fn quote(data: &[u8]) -> String {
    let mut out = String::new();
    for byte in data {
        if byte.is_ascii_alphanumeric() || b"_.-~/".contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `Path.as_uri()` of an absolute path.
pub fn as_uri(path: &Path) -> String {
    let text = paths::display(path);
    if cfg!(windows) {
        let posix = text.replace('\\', "/");
        let bytes = posix.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            return format!("file:///{}{}", &posix[..2], quote(&bytes[2..]));
        }
        return format!("file:{}", quote(bytes));
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        format!("file://{}", quote(path.as_os_str().as_bytes()))
    }
    #[cfg(windows)]
    {
        format!("file://{}", quote(text.as_bytes()))
    }
}

/// A path as a key of a Python dict: `Path` equality is case-insensitive on Windows.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathKey(String);

impl PathKey {
    pub fn of(path: &Path) -> PathKey {
        let text = paths::display(path);
        PathKey(if cfg!(windows) { text.to_lowercase() } else { text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_positions_and_offsets() {
        let text = Text::new("// 😀\r\nab");
        assert_eq!(text.position(4), object([("line", int(0)), ("character", int(5))]));
        let at = |line: usize, character: usize| object([("line", int(line)), ("character", int(character))]);
        assert_eq!(text.offset(&at(0, 5)), Ok(4));
        assert_eq!(text.offset(&at(0, 4)), Err(PyError::Value("position splits a UTF-16 surrogate pair".to_owned())));
        assert_eq!(text.offset(&at(0, 6)), Err(PyError::Value("position is past end of line".to_owned())));
        assert_eq!(text.offset(&at(1, 2)), Ok(8));
        assert_eq!(text.offset(&at(2, 0)), Err(PyError::Value("invalid text position".to_owned())));
    }

    #[test]
    fn uris_split_and_quote_like_python() {
        let path = |uri: &str| local_path(&Value::Str(uri.to_owned()));
        assert_eq!(path("http://x/a"), Err(PyError::Value("a local file URI is required".to_owned())));
        assert_eq!(path("file://host/a"), Err(PyError::Value("remote file hosts are unsupported".to_owned())));
        assert_eq!(path("file:///a?b"), Err(PyError::Value("a local file URI is required".to_owned())));
        assert_eq!(local_path(&Value::Null), Err(PyError::Value("a local file URI is required".to_owned())));
        if cfg!(not(windows)) {
            assert_eq!(path("file:///p%20x/b.copper"), Ok(PathBuf::from("/p x/b.copper")));
            assert_eq!(as_uri(Path::new("/p x/ä.copper")), "file:///p%20x/%C3%A4.copper");
        }
        assert_eq!(decode_drive("/c%3A/x"), "/c:/x");
        assert_eq!(decode_drive("/c%3Ax"), "/c%3Ax");
    }
}
