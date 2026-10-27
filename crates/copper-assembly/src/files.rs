//! Python file-system behaviour the assembly and manufacturing commands
//! report: `Path.mkdir`, text decoding and OS-error text.

use std::path::{Path, PathBuf};

use copper_lang::paths;
use sha2::{Digest, Sha256};

/// `str(OSError)` for `open()` and file reads and writes.
pub fn open_error(error: &std::io::Error, shown: &str) -> String {
    paths::python_os_error(error, Some(shown))
}

/// `str(OSError)` for an `os` function (`mkdir`, `rename`, `rmdir`).
pub fn os_error(error: &std::io::Error, shown: &str) -> String {
    paths::python_os_call_error(error, Some(shown))
}

/// `str(UnicodeDecodeError)` for strict UTF-8 decoding.
pub fn decode_error(bytes: &[u8], error: std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let (end, reason) = match error.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        Some(_) if (0x80..0xc2).contains(&bytes[start]) || bytes[start] > 0xf4 => (start + 1, "invalid start byte"),
        Some(length) => (start + length, "invalid continuation byte"),
    };
    if end - start > 1 {
        format!("'utf-8' codec can't decode bytes in position {start}-{}: {reason}", end - 1)
    } else {
        format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}", bytes[start])
    }
}

/// Text mode's universal newlines: `\r\n` and `\r` read as `\n`.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `Path.parent` for a path built from normalized components: `.` for a bare
/// name.
pub fn parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) => PathBuf::from("."),
        None => path.to_path_buf(),
    }
}

/// `Path.mkdir(parents=..., exist_ok=...)`, reporting the path Python names
/// in its `OSError`.
pub fn mkdir_parents(path: &Path, parents: bool, exist_ok: bool) -> Result<(), String> {
    let path = if path.as_os_str().is_empty() { Path::new(".") } else { path };
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let up = parent(path);
            if !parents || up == path {
                return Err(os_error(&error, &paths::display(path)));
            }
            mkdir_parents(&up, true, true)?;
            mkdir_parents(path, false, exist_ok)
        }
        Err(error) => {
            if !exist_ok || !path.is_dir() {
                return Err(os_error(&error, &paths::display(path)));
            }
            Ok(())
        }
    }
}
