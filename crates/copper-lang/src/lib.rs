//! The CopperScript language frontend.
//!
//! The lexer, parser and syntax tree (originally `pcbir.lexer`, `pcbir.parser`
//! and `pcbir.syntax`): the grammar, the tree and the diagnostics (codes,
//! messages and source locations) of the Python implementation, which
//! CopperScript replaced. Offsets and columns count Unicode code points, as Python
//! strings do.

pub mod antenna;
pub mod bench;
pub mod build_settings;
pub mod compiler;
pub mod diagnostics;
pub mod elaborate;
pub mod emi;
pub mod emi_signals;
pub mod erc;
mod error;
pub mod expression;
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
pub mod noise;
pub mod packages;
pub mod panel;
pub mod panel_presets;
mod parser;
pub mod paths;
pub mod power;
pub mod pyvalue;
pub mod query;
pub mod remote_models;
pub mod serializer;
pub mod silkscreen;
pub mod simulation;
pub mod snapshot;
pub mod spice_models;
mod suggest;
pub mod syntax;
pub mod syntax_state;
pub mod tolerance;
pub mod transient;
pub mod zip_member;

pub use error::{CopperScriptError, Diagnostic};
pub use lexer::{Token, TokenKind, tokenize, tokenize_recovering};
pub use parser::{parse, parse_tokens};
pub use suggest::close_match;
