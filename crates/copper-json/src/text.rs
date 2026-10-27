//! Text output with LF line endings on every platform.
//!
//! Python's text mode (`print`, `Path.write_text`) writes `"\r\n"` on Windows;
//! the Rust binary deliberately writes `"\n"` everywhere (plan, section 4).
//! The comparison harness reads the Python outputs' CRLF as LF on Windows.

use std::io::{self, Write};
use std::path::Path;

/// Python's `print(text)`: the text and a newline.
pub fn print(out: &mut (impl Write + ?Sized), text: &str) -> io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")
}

/// Python's `Path.write_text(text, encoding="utf-8")`, with LF line endings.
pub fn write_text(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text.as_bytes())
}
