//! Deterministic engineering summaries of full reports (`pcbir.engineering_report`).

use std::path::Path;

use copper_pcb::Result;

/// `write_summary`: the summary file beside the report, and its `summary_lines`.
pub fn write_summary(_report: &Path, _output: &Path) -> Result<Vec<String>> {
    Ok(Vec::new())
}
