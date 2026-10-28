//! Pin access, planes, the routing pipeline and the benchmark ladder
//! (`pcbir.fanout`, `decoupling`, `decoupling_paths`, `plane`,
//! `filled_feedback`, `dependency_reroute`, `flow`, `routing_acceptance`,
//! `progress`, `ladder`, `routing_benchmark` and `bench`), the M4 "flow"
//! stream of the Rust rewrite (docs/rust-port-plan.md).
//!
//! One Rust module per Python module, with the same names, dataclass fields
//! and function signatures, so the Python source can be read alongside. The
//! global, unified and critical routers are owned by other streams; where this
//! crate needs one that has not landed, the stage reports
//! [`copper_pcb::Error::NotImplemented`] rather than a wrong answer.

pub mod drc_report;
pub mod ladder;
pub mod plane_verify;
pub mod progress;
pub mod pypath;
pub mod routing_acceptance;
pub mod routing_metrics;

pub use progress::{Progress, emit};
pub use pypath::display as pypath_display;

/// `sys.version` of the acceptance report: this implementation is not Python,
/// so the report names the rewrite instead.
pub fn not_python_version() -> String {
    format!("copper {} (Rust rewrite)", env!("CARGO_PKG_VERSION"))
}

/// `platform.platform()` of the acceptance report, as far as a Rust binary knows it.
pub fn platform_text() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// `_implementation_digest()`: Python digests its own sources; the binary can
/// only name the version it was built as.
pub fn implementation_digest() -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(not_python_version().as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}
