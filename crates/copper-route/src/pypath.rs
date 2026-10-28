//! `str(pathlib.Path(arg))` for path arguments, as the binary's `pypath` shows them.

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
