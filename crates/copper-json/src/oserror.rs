//! Python's `str(OSError)` for a failed file-system call.
//!
//! On POSIX both kinds of call read `[Errno N] strerror: 'name'`. On Windows
//! Python reports the C runtime's view for `open()` (the Win32 code mapped to
//! an errno by `_dosmaperr`, with the CRT's message) and the Win32 code itself
//! for `os` functions such as `mkdir`, `unlink` and `lstat`.

use crate::python_str_repr;

/// Which Python layer the failing call goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// `open()`, `Path.read_bytes`, `Path.write_text` and friends.
    Open,
    /// An `os` module function (`os.mkdir`, `os.unlink`, `os.lstat`, ...).
    Os,
}

/// `str(error)` as Python prints it, with `filename` when Python attaches one.
pub fn python_os_error(error: &std::io::Error, filename: Option<&str>, call: Call) -> String {
    let text = error.to_string();
    let message = text.rfind(" (os error ").map_or(text.as_str(), |index| &text[..index]);
    let Some(code) = error.raw_os_error() else { return message.to_owned() };
    let head = if cfg!(windows) {
        match call {
            Call::Open => {
                let errno = dos_errno(code);
                format!("[Errno {errno}] {}", crt_strerror(errno))
            }
            // Python trims trailing whitespace and dots from FormatMessage text.
            Call::Os => format!("[WinError {code}] {}", message.trim_end_matches(|c: char| c == '.' || c <= ' ')),
        }
    } else {
        format!("[Errno {code}] {message}")
    };
    match filename {
        Some(name) => format!("{head}: {}", python_str_repr(name)),
        None => head,
    }
}

/// The C runtime's `_dosmaperr`: a Win32 error code as an errno.
fn dos_errno(code: i32) -> i32 {
    const ENOENT: i32 = 2;
    const E2BIG: i32 = 7;
    const ENOEXEC: i32 = 8;
    const EBADF: i32 = 9;
    const ECHILD: i32 = 10;
    const EAGAIN: i32 = 11;
    const ENOMEM: i32 = 12;
    const EACCES: i32 = 13;
    const EEXIST: i32 = 17;
    const EXDEV: i32 = 18;
    const EINVAL: i32 = 22;
    const EMFILE: i32 = 24;
    const ENOSPC: i32 = 28;
    const EPIPE: i32 = 32;
    const ENOTEMPTY: i32 = 41;
    match code {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => ENOENT,
        4 => EMFILE,
        5 | 16 | 19..=36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => EACCES,
        6 | 114 | 130 => EBADF,
        7..=9 | 1816 => ENOMEM,
        10 => E2BIG,
        11 | 188..=202 => ENOEXEC,
        17 => EXDEV,
        80 | 183 => EEXIST,
        89 | 164 | 215 => EAGAIN,
        109 => EPIPE,
        112 => ENOSPC,
        128 | 129 => ECHILD,
        145 => ENOTEMPTY,
        _ => EINVAL,
    }
}

/// The Universal C runtime's `strerror` text.
fn crt_strerror(errno: i32) -> &'static str {
    match errno {
        1 => "Operation not permitted",
        2 => "No such file or directory",
        3 => "No such process",
        4 => "Interrupted function call",
        5 => "Input/output error",
        6 => "No such device or address",
        7 => "Arg list too long",
        8 => "Exec format error",
        9 => "Bad file descriptor",
        10 => "No child processes",
        11 => "Resource temporarily unavailable",
        12 => "Not enough space",
        13 => "Permission denied",
        14 => "Bad address",
        16 => "Resource device",
        17 => "File exists",
        18 => "Improper link",
        19 => "No such device",
        20 => "Not a directory",
        21 => "Is a directory",
        22 => "Invalid argument",
        23 => "Too many open files in system",
        24 => "Too many open files",
        25 => "Inappropriate I/O control operation",
        27 => "File too large",
        28 => "No space left on device",
        29 => "Invalid seek",
        30 => "Read-only file system",
        31 => "Too many links",
        32 => "Broken pipe",
        33 => "Domain error",
        34 => "Result too large",
        36 => "Resource deadlock avoided",
        38 => "Filename too long",
        39 => "No locks available",
        40 => "Function not implemented",
        41 => "Directory not empty",
        42 => "Illegal byte sequence",
        _ => "Unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_like_python() {
        let error = std::fs::read("definitely-missing.kicad_mod").unwrap_err();
        assert_eq!(
            python_os_error(&error, Some("definitely-missing.kicad_mod"), Call::Open),
            "[Errno 2] No such file or directory: 'definitely-missing.kicad_mod'"
        );
    }

    #[cfg(windows)]
    #[test]
    fn os_calls_report_the_win32_code() {
        let error = std::io::Error::from_raw_os_error(183);
        assert_eq!(
            python_os_error(&error, Some(r"a\b"), Call::Os),
            r"[WinError 183] Cannot create a file when that file already exists: 'a\\b'"
        );
        assert_eq!(python_os_error(&error, None, Call::Open), "[Errno 17] File exists");
    }
}
