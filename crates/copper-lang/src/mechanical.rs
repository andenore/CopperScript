//! Mechanical design lowering (`pcbir.design.lower_mechanical`).
//!
//! Not rewritten yet: a board with a `mechanical` block reports
//! [`NOT_IMPLEMENTED`] so the CLI answers "not implemented" instead of
//! producing output that silently lacks its mechanics.

use indexmap::IndexMap;

use crate::compiler::MechanicalProfileDefinition;
use crate::error::CopperScriptError;
use crate::syntax::{Declaration, Document, Location};

/// The code of the placeholder error for rewrite gaps.
pub const NOT_IMPLEMENTED: &str = "RUST_NOT_IMPLEMENTED";

#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalDesign {}

impl MechanicalDesign {
    /// Component references bound by connectors, attachments and assembly items.
    pub fn bound_references(&self) -> Vec<(String, Option<&Location>)> {
        Vec::new()
    }
}

pub fn lower_mechanical(
    document: &Document,
    _profiles: &IndexMap<String, MechanicalProfileDefinition>,
) -> Result<Option<MechanicalDesign>, CopperScriptError> {
    match document.declarations.iter().find(|declaration| matches!(declaration, Declaration::Mechanical(_))) {
        None => Ok(None),
        Some(Declaration::Mechanical(block)) => {
            Err(CopperScriptError::new(NOT_IMPLEMENTED, "mechanical blocks (M1, in progress)", block.location.clone()))
        }
        Some(_) => unreachable!(),
    }
}
