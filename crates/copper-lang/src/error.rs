//! Source-located diagnostics (`pcbir.syntax.CopperScriptError`).

use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::syntax::Location;

/// The fields of a [`CopperScriptError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub location: Location,
    pub hint: Option<String>,
    /// Machine-applicable replacement for the token at `location`.
    pub replacement: Option<String>,
}

/// A lexical, syntax or lowering error at a source location. Boxed so that
/// `Result`s stay small; the fields are reached through `Deref`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopperScriptError(Box<Diagnostic>);

impl CopperScriptError {
    pub fn new(code: &str, message: impl Into<String>, location: Location) -> Self {
        CopperScriptError(Box::new(Diagnostic {
            code: code.to_owned(),
            message: message.into(),
            location,
            hint: None,
            replacement: None,
        }))
    }
}

impl Deref for CopperScriptError {
    type Target = Diagnostic;
    fn deref(&self) -> &Diagnostic {
        &self.0
    }
}

impl DerefMut for CopperScriptError {
    fn deref_mut(&mut self) -> &mut Diagnostic {
        &mut self.0
    }
}

/// Python's `str(error)`: `"file:line:column: CODE: message (hint)"`.
impl fmt::Display for CopperScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}: {}", self.location, self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(formatter, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for CopperScriptError {}
