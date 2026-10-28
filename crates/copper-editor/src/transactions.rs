//! `pcbir.editor.transactions`: the electrical identity an edit must keep,
//! session seeds, and the reviewed source review/save transaction.

use copper_lang::model::{Board, Constraint, ConstraintKind, MacroBinding};
use copper_lang::syntax::Location;

fn neutral(location: &Location) -> Location {
    Location { filename: location.filename.clone(), offset: 0, line: 0, column: 0 }
}

/// `macro_binding_identity`: ignore only diagnostic offsets.
pub fn macro_binding_identity(binding: &MacroBinding) -> MacroBinding {
    let mut identity = binding.clone();
    identity.location = neutral(&binding.location);
    identity.asset.location = neutral(&binding.asset.location);
    identity.asset.locked = false;
    identity.asset.offline = false;
    for mapping in &mut identity.mappings {
        mapping.location = neutral(&mapping.location);
    }
    identity
}

fn without_origins(constraints: &[Constraint], keep_fixed: bool) -> Vec<Constraint> {
    constraints
        .iter()
        .filter(|c| keep_fixed || c.kind != ConstraintKind::FixedPlacement)
        .map(|c| Constraint { origins: Vec::new(), ..c.clone() })
        .collect()
}

/// `electrical_identity`: ignore only editable pose intent and diagnostic
/// source offsets. Every part, hierarchy, configuration, net, supply,
/// interface, dependency and other constraint must remain exactly equal.
pub fn electrical_identity(board: &Board) -> Board {
    let mut identity = board.clone();
    identity.constraints = without_origins(&board.constraints, false);
    for module in identity.module_definitions.values_mut() {
        module.constraints = without_origins(&module.constraints, true);
        module.physical_macros = module.physical_macros.iter().map(macro_binding_identity).collect();
    }
    identity.physical_macros = board.physical_macros.iter().map(macro_binding_identity).collect();
    identity
}
