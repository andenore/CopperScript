//! Artifact-bound verification of individual semantic declarations
//! (`pcbir.requirement_verification`), placement stage.

use copper_json::Value;
use copper_lang::compiler::Design;
use copper_pcb::Result;
use copper_pcb::board::PhysicalBoard;

/// `verify_requirements(source, physical, stage=stage)`.
pub fn verify_requirements(_design: &Design, _board: &PhysicalBoard, _stage: &str) -> Result<Value> {
    let _ = Value::Null;
    copper_pcb::not_implemented("placement requirement verification")
}
