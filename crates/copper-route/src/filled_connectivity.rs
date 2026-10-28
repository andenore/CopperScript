//! Conservative component grouping for native-fill observations
//! (`pcbir.filled_connectivity`).
//!
//! Bare copper contacts are shared with DRC; source-zone outlines and
//! installed-part internal edges do not join components. Joining native fill
//! fragments needs exact polygon overlap areas (shapely), which is not
//! rewritten: a board whose evidence holds fill fragments of the nets exits 3.

use std::collections::BTreeMap;

use indexmap::IndexSet;

use copper_drc::connectivity::{Candidate, copper_contact_roots, explicit_copper_contacts};
use copper_pcb::board::PhysicalBoard;

use crate::Result;
use crate::plane_verify::{NativeCopperItem, PlaneVerification};

/// `native_item_identity`: the contact identity of a native object, if any.
pub fn native_item_identity(item: &NativeCopperItem) -> Option<String> {
    match item.kind.as_str() {
        "pad" => Some(format!(
            "pad:{}.{}:{}",
            item.component.as_deref().unwrap_or("None"),
            item.pad.as_deref().unwrap_or("None"),
            item.index.map_or_else(|| "None".to_owned(), |i| i.to_string())
        )),
        "segment" => Some(format!("track:{}", item.index.map_or_else(|| "None".to_owned(), |i| i.to_string()))),
        "via" => Some(format!("via:{}", item.index.map_or_else(|| "None".to_owned(), |i| i.to_string()))),
        _ => None,
    }
}

/// `filled_component_roots(board, evidence, nets)`.
pub fn filled_component_roots(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
    nets: &IndexSet<String>,
) -> Result<BTreeMap<String, String>> {
    let candidate = Candidate { only_nets: Some(nets), exclude_internal_connections: true, ..Candidate::default() };
    let (contacts, _, groups) = explicit_copper_contacts(board, candidate)?;
    let parent = copper_contact_roots(&contacts, &groups)?;
    if !evidence.matches(board)? {
        return Ok(parent);
    }
    if evidence.filled_regions.iter().any(|fragment| nets.contains(&fragment.net)) {
        return copper_pcb::not_implemented("native fill fragment contacts (filled_connectivity, shapely areas)");
    }
    Ok(parent)
}
