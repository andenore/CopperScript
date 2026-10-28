//! The physical editor (`pcbir.editor`): the routed editor overlay, the
//! derived scene with its pad-island ratsnest, revision-bound placement
//! previews, reviewed source transactions and the loopback HTTP transport.
//!
//! Errors carry Python's exception class, because the command handlers and
//! the HTTP transport answer each class differently: `ValueError` (and its
//! editor subclasses) and `OSError` become reported errors, `StaleRevision`
//! is a 409, a `CopperScriptError` prints with its code and location, and
//! anything Python would not catch is an uncaught crash.

pub mod artwork;
pub mod configuration;
pub mod definitions;
pub mod dependencies;
pub mod features;
pub mod islands;
pub mod jobs;
pub mod overlay;
pub mod project;
pub mod scene;
pub mod server;
pub mod session;
pub mod source;
pub mod transactions;

use std::fmt;

use copper_lang::CopperScriptError;

/// The Python exception an operation raised, by class.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// `ValueError`, `EditorError` and `SourceEditError` (by message).
    Value(String),
    /// `StaleRevision`: the session or source moved on (HTTP 409).
    Stale(String),
    /// `OSError` text as Python prints it.
    Os(String),
    /// A located compiler diagnostic.
    Copper(CopperScriptError),
    /// Behaviour not rewritten yet (exit 3 in the command line).
    NotImplemented(String),
    /// An exception Python does not catch on this path: `Class: message`.
    Uncaught(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Value(message)
            | Error::Stale(message)
            | Error::Os(message)
            | Error::NotImplemented(message)
            | Error::Uncaught(message) => formatter.write_str(message),
            Error::Copper(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<copper_pcb::Error> for Error {
    fn from(error: copper_pcb::Error) -> Self {
        match error {
            copper_pcb::Error::Value(message) => Error::Value(message),
            copper_pcb::Error::NotImplemented(message) => Error::NotImplemented(message),
        }
    }
}

impl From<CopperScriptError> for Error {
    fn from(error: CopperScriptError) -> Self {
        Error::Copper(error)
    }
}

/// A `ValueError` with this message.
pub fn value<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

/// The hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `[x_nm, y_nm]`.
pub(crate) fn point_value(point: copper_physical::Point) -> copper_json::Value {
    copper_json::Value::Array(vec![copper_json::Value::from(point.x_nm), copper_json::Value::from(point.y_nm)])
}

/// A JSON object in this key order.
pub(crate) fn object(entries: Vec<(&str, copper_json::Value)>) -> copper_json::Value {
    copper_json::Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}
