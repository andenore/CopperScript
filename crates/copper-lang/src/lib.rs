//! The CopperScript language frontend.
//!
//! A rewrite of `pcbir.lexer`, `pcbir.parser` and `pcbir.syntax`: the same
//! grammar, the same tree and the same diagnostics (codes, messages and
//! source locations), so `copper check` reports parse errors exactly as the
//! Python CLI does. Offsets and columns count Unicode code points, as Python
//! strings do.

mod error;
mod lexer;
mod parser;
mod suggest;
pub mod syntax;

pub use error::{CopperScriptError, Diagnostic};
pub use lexer::{Token, TokenKind, tokenize};
pub use parser::parse;
pub use suggest::close_match;
