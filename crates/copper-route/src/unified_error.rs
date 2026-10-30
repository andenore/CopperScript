//! Errors of the unified-router stream (`replay-unified`): a caught
//! `ValueError`/`StateError` (`REPLAY ERROR`), an uncaught Python exception
//! (traceback, exit 1), or behaviour not implemented yet (exit 3).

/// How a Python call ends: a caught `ValueError`/`StateError`/`OSError`
/// (`REPLAY ERROR: message`), an uncaught exception (a traceback, exit 1),
/// or behaviour this implementation does not implement (exit 3, never a wrong answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A `ValueError` (or `StateError`, `OSError`) by its message.
    Value(String),
    /// An uncaught exception: its class (as the traceback's last line names
    /// it) and `str(exception)`.
    Crash(String, String),
    NotImplemented(String),
}

impl Error {
    pub fn value(message: impl Into<String>) -> Self {
        Error::Value(message.into())
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Error::Crash("TypeError".to_owned(), message.into())
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        Error::NotImplemented(what.into())
    }

    /// The last line of Python's traceback for a crash.
    pub fn crash_line(&self) -> String {
        match self {
            Error::Crash(kind, message) if message.is_empty() => kind.clone(),
            Error::Crash(kind, message) => format!("{kind}: {message}"),
            Error::Value(message) => format!("ValueError: {message}"),
            Error::NotImplemented(what) => what.clone(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Value(message) | Error::NotImplemented(message) => formatter.write_str(message),
            Error::Crash(kind, message) => write!(formatter, "{kind}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<copper_pcb::Error> for Error {
    fn from(error: copper_pcb::Error) -> Self {
        match error {
            copper_pcb::Error::Value(message) => Error::Value(message),
            copper_pcb::Error::NotImplemented(message) => Error::NotImplemented(message),
        }
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::Value(message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

pub fn not_implemented<T>(what: impl Into<String>) -> Result<T> {
    Err(Error::NotImplemented(what.into()))
}
