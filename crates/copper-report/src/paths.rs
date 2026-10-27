//! `pathlib` path text and the file reads the Python code makes.
//!
//! Paths are kept as the text `str(Path(...))` would show, so messages and
//! documents print them the same way; the file system is reached through
//! that text.

use std::path::Path;

use copper_json::Value;
use copper_json::oserror::{Call, python_os_error};

use crate::decode::decode_utf8;
use crate::py::{Py, PyErr};

pub const SEPARATOR: char = std::path::MAIN_SEPARATOR;

fn is_separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

/// `str(Path(text))` on this platform: separators normalized, `.` parts and
/// repeated or trailing separators removed, `"."` for an empty path.
pub fn display(text: &str) -> String {
    if cfg!(windows) {
        let unified = text.replace('/', "\\");
        let (drive, rest) = match unified.as_bytes() {
            [letter, b':', ..] if letter.is_ascii_alphabetic() => unified.split_at(2),
            _ => ("", unified.as_str()),
        };
        let root = if rest.starts_with('\\') { "\\" } else { "" };
        let parts: Vec<&str> = rest.split('\\').filter(|part| !part.is_empty() && *part != ".").collect();
        let joined = format!("{drive}{root}{}", parts.join("\\"));
        if joined.is_empty() { ".".to_owned() } else { joined }
    } else {
        let leading = text.len() - text.trim_start_matches('/').len();
        // POSIX keeps exactly two leading slashes; one or three-plus collapse to one.
        let root = match leading {
            0 => "",
            2 => "//",
            _ => "/",
        };
        let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
        let joined = format!("{root}{}", parts.join("/"));
        if joined.is_empty() { ".".to_owned() } else { joined }
    }
}

/// `Path(directory) / name`.
pub fn join(directory: &str, name: &str) -> String {
    if directory == "." || directory.is_empty() {
        return name.to_owned();
    }
    if name.starts_with(is_separator) {
        return name.to_owned();
    }
    if directory.ends_with(is_separator) {
        format!("{directory}{name}")
    } else {
        format!("{directory}{SEPARATOR}{name}")
    }
}

/// `PurePath.name`.
pub fn name(path: &str) -> &str {
    path.rsplit(is_separator).next().unwrap_or(path)
}

/// `PurePath.parent`.
pub fn parent(path: &str) -> String {
    match path.rfind(is_separator) {
        None if cfg!(windows) && path.len() >= 2 && path.as_bytes()[1] == b':' => path[..2].to_owned(),
        None => ".".to_owned(),
        Some(index) => {
            let head = path[..index].trim_end_matches(is_separator);
            let drive = cfg!(windows) && head.len() == 2 && head.as_bytes()[1] == b':';
            if head.is_empty() || drive { path[..=index].to_owned() } else { head.to_owned() }
        }
    }
}

/// `PurePath.suffix`.
pub fn suffix(path: &str) -> &str {
    let name = name(path);
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

/// `PurePath.stem`.
pub fn stem(path: &str) -> &str {
    let name = name(path);
    let suffix = suffix(path);
    &name[..name.len() - suffix.len()]
}

/// `PurePath.with_name(name)`.
pub fn with_name(path: &str, name: &str) -> String {
    join(&parent(path), name)
}

pub fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// `sorted(Path(directory).glob("*" + suffix))`: entries of the directory
/// (hidden ones too) whose name ends with `suffix`, as joined paths.
pub fn glob_suffix(directory: &str, suffix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(if directory.is_empty() { "." } else { directory }) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(suffix))
        .collect();
    // Paths compare by their parts, case-folded on Windows.
    if cfg!(windows) {
        names.sort_by_key(|name| name.to_lowercase());
    } else {
        names.sort();
    }
    names.into_iter().map(|name| join(directory, &name)).collect()
}

/// `Path.read_bytes()`.
pub fn read_bytes(path: &str) -> Py<Vec<u8>> {
    std::fs::read(Path::new(path)).map_err(|error| os_error(&error, path))
}

pub fn os_error(error: &std::io::Error, path: &str) -> PyErr {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    };
    PyErr::new(kind, python_os_error(error, Some(path), Call::Open))
}

/// `Path.read_text(encoding="utf-8")`: strict UTF-8 with universal newlines.
pub fn read_text(path: &str) -> Py<String> {
    let bytes = read_bytes(path)?;
    let text = decode_utf8(&bytes)?;
    Ok(if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text })
}

/// `json.loads(text)` for a `str` document.
pub fn loads(text: &str) -> Py<Value> {
    if text.starts_with('\u{feff}') {
        return Err(PyErr::new(
            "JSONDecodeError",
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
        ));
    }
    copper_json::parse(text).map_err(|error| PyErr::new("JSONDecodeError", error.python_message(text)))
}

/// `hashlib.sha256(Path(path).read_bytes()).hexdigest()` when it is a file.
pub fn sha256_file(path: Option<&str>) -> Option<String> {
    use sha2::{Digest, Sha256};
    let path = path.filter(|path| is_file(path))?;
    let bytes = std::fs::read(Path::new(path)).ok()?;
    Some(Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `Path.stat().st_mtime`: seconds as a float, as CPython computes it.
pub fn mtime(path: &str) -> Py<f64> {
    let metadata = std::fs::metadata(Path::new(path)).map_err(|error| os_error(&error, path))?;
    let modified = metadata.modified().map_err(|error| os_error(&error, path))?;
    let (seconds, nanos) = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => (duration.as_secs() as f64, f64::from(duration.subsec_nanos())),
        Err(before) => {
            let duration = before.duration();
            let (mut seconds, mut nanos) = (-(duration.as_secs() as i64), -i64::from(duration.subsec_nanos()));
            if nanos < 0 {
                seconds -= 1;
                nanos += 1_000_000_000;
            }
            (seconds as f64, nanos as f64)
        }
    };
    Ok(seconds + nanos * 1e-9)
}

/// `Path.resolve()` (non-strict): absolute, symbolic links resolved where
/// the path exists, `..` applied lexically elsewhere.
pub fn resolve(path: &str) -> String {
    let absolute = std::path::absolute(Path::new(path)).unwrap_or_else(|_| Path::new(path).to_path_buf());
    let mut current = std::path::PathBuf::new();
    for component in absolute.components() {
        use std::path::Component;
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(part) => {
                let candidate = current.join(part);
                current = std::fs::canonicalize(&candidate).unwrap_or(candidate);
            }
        }
    }
    let text = current.to_string_lossy().into_owned();
    // `canonicalize` spells Windows paths in their verbatim form.
    text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(text)
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn pathlib_parts() {
        assert_eq!(join(".", "a.json"), "a.json");
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("run", "a"), "run/a");
        assert_eq!(parent("a.json"), ".");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("run/sub/a.json"), "run/sub");
        assert_eq!(stem("run/route-report.json"), "route-report");
        assert_eq!(suffix("run/.json"), "");
        assert_eq!(with_name("run/route-report.json", "x.json"), "run/x.json");
    }
}
