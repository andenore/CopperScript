//! The CopperScript language frontend.
//!
//! A rewrite of `pcbir.lexer`, `pcbir.parser` and `pcbir.syntax`: the same
//! grammar, the same tree and the same diagnostics (codes, messages and
//! source locations), so `copper check` reports parse errors exactly as the
//! Python CLI does. Offsets and columns count Unicode code points, as Python
//! strings do.

pub mod compiler;
pub mod diagnostics;
pub mod elaborate;
pub mod erc;
mod error;
mod lexer;
pub mod library;
pub mod lower;
pub mod mechanical;
pub mod model;
pub mod modes;
pub mod packages;
mod parser;
pub mod paths;
pub mod power;
pub mod pyvalue;
pub mod query;
pub mod serializer;
pub mod silkscreen;
mod suggest;
pub mod syntax;

pub use error::{CopperScriptError, Diagnostic};
pub use lexer::{Token, TokenKind, tokenize, tokenize_recovering};
pub use parser::parse;
pub use suggest::close_match;
