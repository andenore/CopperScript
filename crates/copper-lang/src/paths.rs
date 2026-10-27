//! Python `pathlib` and `os.path` semantics the frontend depends on.
//!
//! Package resolution compares resolved absolute paths and prints them in
//! diagnostics, so these mirror Python: `Path.resolve()` (non-strict),
//! `str(Path)`, `as_posix()` and `os.path.relpath`.

use std::path::{Component, Path, PathBuf};

/// Remove the Windows verbatim prefix `canonicalize` adds (`\\?\C:\x` is
/// `C:\x` to Python; `\\?\UNC\host\share` is `\\host\share`).
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

fn lexical_push(path: &mut PathBuf, component: Component<'_>) {
    match component {
        Component::CurDir => {}
        Component::ParentDir => {
            path.pop();
        }
        other => path.push(other.as_os_str()),
    }
}

/// Python's `Path(path).resolve()`: absolute, symlinks resolved for the part
/// that exists, the rest normalized lexically.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    if let Ok(resolved) = std::fs::canonicalize(&absolute) {
        return strip_verbatim(resolved);
    }
    let components: Vec<Component<'_>> = absolute.components().collect();
    for split in (0..components.len()).rev() {
        let mut prefix = PathBuf::new();
        for component in &components[..split] {
            prefix.push(component.as_os_str());
        }
        if let Ok(existing) = std::fs::canonicalize(&prefix) {
            let mut resolved = strip_verbatim(existing);
            for component in &components[split..] {
                lexical_push(&mut resolved, *component);
            }
            return resolved;
        }
    }
    let mut resolved = PathBuf::new();
    for component in components {
        lexical_push(&mut resolved, component);
    }
    resolved
}

/// Python's `str(path)` for a path built from native components.
pub fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Python's `PurePath.as_posix()`.
pub fn as_posix(path: &Path) -> String {
    let text = display(path);
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}

/// Python's `PurePath.is_relative_to(base)` (component-wise).
pub fn is_relative_to(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

/// Python's `os.path.relpath(path, start)` for two absolute, resolved paths.
pub fn relpath(path: &Path, start: &Path) -> PathBuf {
    let target: Vec<Component<'_>> = path.components().collect();
    let base: Vec<Component<'_>> = start.components().collect();
    let common = target.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut relative = PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() { PathBuf::from(".") } else { relative }
}

/// Python's `os.path.splitext(name)[1]`: the extension, or `""` when the
/// only dots lead the name (`.json`).
pub fn extension(name: &str) -> &str {
    let Some(dot) = name.rfind('.') else { return "" };
    if name[..dot].chars().all(|character| character == '.') { "" } else { &name[dot..] }
}

/// Python's `str(OSError)`: `"[Errno 2] No such file or directory: 'path'"`.
pub fn python_os_error(error: &std::io::Error, filename: Option<&str>) -> String {
    let text = error.to_string();
    let message = match text.rfind(" (os error ") {
        Some(index) => text[..index].to_owned(),
        None => text,
    };
    match (error.raw_os_error(), filename) {
        (Some(code), Some(name)) => format!("[Errno {code}] {message}: {}", copper_json::python_str_repr(name)),
        (Some(code), None) => format!("[Errno {code}] {message}"),
        (None, _) => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitext_and_relpath_follow_python() {
        assert_eq!(extension("a.copper"), ".copper");
        assert_eq!(extension(".json"), "");
        assert_eq!(extension("..a.b"), ".b");
        assert_eq!(extension("Makefile"), "");
        let root = if cfg!(windows) { PathBuf::from(r"C:\a\b") } else { PathBuf::from("/a/b") };
        assert_eq!(relpath(&root.join("c/d"), &root.join("e")), PathBuf::from("../c/d"));
        assert_eq!(relpath(&root, &root), PathBuf::from("."));
    }
}
