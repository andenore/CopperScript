//! Python text-mode newline translation.
//!
//! Python writes text (`print`, `Path.write_text`, `open(..., "w")`) with
//! universal newlines: on Windows every `"\n"` becomes `"\r\n"`, elsewhere the
//! text is unchanged. The binary reproduces that so its stdout and text files
//! match the Python CLI byte for byte on each platform.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;

/// `text` as Python's text mode would write it on this platform.
pub fn platform_newlines(text: &str) -> Cow<'_, str> {
    if cfg!(windows) && text.contains('\n') { Cow::Owned(text.replace('\n', "\r\n")) } else { Cow::Borrowed(text) }
}

/// Python's `print(text)`: the text and a newline, in text mode.
pub fn print(out: &mut (impl Write + ?Sized), text: &str) -> io::Result<()> {
    out.write_all(platform_newlines(text).as_bytes())?;
    out.write_all(platform_newlines("\n").as_bytes())
}

/// Python's `Path.write_text(text, encoding="utf-8")`.
pub fn write_text(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, platform_newlines(text).as_bytes())
}
