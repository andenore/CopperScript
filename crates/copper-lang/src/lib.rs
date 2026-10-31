//! The CopperScript language frontend.
//!
//! The lexer, parser and syntax tree (originally `pcbir.lexer`, `pcbir.parser`
//! and `pcbir.syntax`): the grammar, the tree and the diagnostics (codes,
//! messages and source locations) of the Python implementation, which
//! CopperScript replaced. Offsets and columns count Unicode code points, as Python
//! strings do.

pub mod compiler;
pub mod diagnostics;
pub mod elaborate;
pub mod erc;
mod error;
pub mod footprints;
pub mod kicad_schematic;
mod lexer;
pub mod library;
pub mod library_context;
pub mod lower;
pub mod macros;
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
pub mod snapshot;
mod suggest;
pub mod syntax;
pub mod syntax_state;

pub use error::{CopperScriptError, Diagnostic};
pub use lexer::{Token, TokenKind, tokenize, tokenize_recovering};
pub use parser::{parse, parse_tokens};
pub use suggest::close_match;
