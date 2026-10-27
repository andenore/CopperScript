//! The CopperScript language server (`copper lsp`, `pcbir.lsp`).
//!
//! LSP over stdio, locked and offline: the same framing, JSON bodies and
//! message order as the Python server for the same client messages. Analysis
//! reuses the `copper-lang` frontend; editor overlays never touch locks or
//! build artifacts.

pub mod analysis;
pub mod frontend;
pub mod pyerr;
pub mod server;
pub mod text;

pub use server::serve;
