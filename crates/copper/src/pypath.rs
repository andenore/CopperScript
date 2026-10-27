//! How Python shows paths in messages: `str(pathlib.Path(arg))` and `repr(str)`.

/// `str(Path(arg))` on this platform: separators normalized, `.` components
/// and repeated or trailing separators removed, `"."` for an empty path.
pub fn display(arg: &str) -> String {
    if cfg!(windows) { windows(arg) } else { posix(arg) }
}

fn posix(arg: &str) -> String {
    let leading = arg.len() - arg.trim_start_matches('/').len();
    // POSIX keeps exactly two leading slashes; one or three-plus collapse to one.
    let root = match leading {
        0 => "",
        2 => "//",
        _ => "/",
    };
    let parts: Vec<&str> = arg.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    let joined = format!("{root}{}", parts.join("/"));
    if joined.is_empty() { ".".to_owned() } else { joined }
}

fn windows(arg: &str) -> String {
    let unified = arg.replace('/', "\\");
    let (drive, rest) = match unified.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => unified.split_at(2),
        _ => ("", unified.as_str()),
    };
    let root = if rest.starts_with('\\') { "\\" } else { "" };
    let parts: Vec<&str> = rest.split('\\').filter(|part| !part.is_empty() && *part != ".").collect();
    let joined = format!("{drive}{root}{}", parts.join("\\"));
    if joined.is_empty() { ".".to_owned() } else { joined }
}

/// Python's `repr(str)`.
pub fn repr(text: &str) -> String {
    copper_json::python_str_repr(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_paths_normalize_like_pathlib() {
        assert_eq!(posix("./a//b/./c/"), "a/b/c");
        assert_eq!(posix(""), ".");
        assert_eq!(posix("//net/x"), "//net/x");
        assert_eq!(posix("///x"), "/x");
        assert_eq!(posix("a/../b"), "a/../b");
    }

    #[test]
    fn windows_paths_normalize_like_pathlib() {
        assert_eq!(windows("crates/x/./y.json"), "crates\\x\\y.json");
        assert_eq!(windows("C:/a//b/"), "C:\\a\\b");
        assert_eq!(repr("crates\\x"), "'crates\\\\x'");
        assert_eq!(repr("it's"), "\"it's\"");
    }
}
