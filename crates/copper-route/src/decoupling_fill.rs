//! Post-fill continuity observations of declared bypasses
//! (`pcbir.decoupling_fill`).
//!
//! These observations never authorize removal of local routes, waive bypass
//! gates, or qualify a high-frequency current loop. Native DRC stays
//! independent.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PadReference, PhysicalBoard};

use crate::Result;
use crate::filled_connectivity::{filled_component_roots, native_item_identity};
use crate::plane_verify::{
    DECOUPLING_FILL_QUALIFICATION, DecouplingFillAssessment, PlaneVerification, native_object_map,
};

fn assessment(
    link: &copper_pcb::board::DecouplingLink,
    status: &str,
    supply: Option<bool>,
    returned: Option<bool>,
    targets: Vec<PadReference>,
    reason: &str,
) -> DecouplingFillAssessment {
    DecouplingFillAssessment {
        link: link.clone(),
        status: status.to_owned(),
        supply_connected: supply,
        return_connected: returned,
        return_targets: targets,
        reason: reason.to_owned(),
        qualification: DECOUPLING_FILL_QUALIFICATION.to_owned(),
    }
}

/// `assess_decoupling_fill(board, evidence)`: declared pin-associated
/// bypasses observed on one exact native-filled export.
pub fn assess_decoupling_fill(
    board: &PhysicalBoard,
    evidence: &PlaneVerification,
) -> Result<Vec<DecouplingFillAssessment>> {
    let links = &board.decoupling_links;
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let unsupported = |reason: &str| -> Vec<DecouplingFillAssessment> {
        links.iter().map(|link| assessment(link, "unsupported", None, None, Vec::new(), reason)).collect()
    };
    if !evidence.matches(board)? {
        return Ok(unsupported("stale_source_or_export"));
    }
    if evidence.filled_board_digest.is_empty() || evidence.report_digest.is_empty() {
        return Ok(unsupported("missing_native_artifact_binding"));
    }
    if evidence
        .native_findings
        .iter()
        .filter(|finding| finding.category == "unconnected")
        .any(|finding| finding.items.iter().any(|item| item.kind == "unknown"))
    {
        return Ok(unsupported("unmapped_native_object"));
    }
    let nets: IndexSet<String> = links.iter().flat_map(|link| [link.net.clone(), link.return_net.clone()]).collect();
    let roots = filled_component_roots(board, evidence, &nets)?;
    let mut terminals: IndexMap<PadReference, Vec<crate::plane_verify::NativeCopperItem>> = IndexMap::new();
    for item in native_object_map(board)?.into_values() {
        if item.kind == "pad" {
            let reference = PadReference {
                component: item.component.clone().unwrap_or_default(),
                pad: item.pad.clone().unwrap_or_default(),
            };
            terminals.entry(reference).or_default().push(item);
        }
    }
    let identity = |reference: &PadReference, net: &str| -> Option<String> {
        let items = terminals.get(reference)?;
        if items.len() != 1 || items[0].net.as_deref() != Some(net) {
            return None;
        }
        native_item_identity(&items[0]).filter(|key| roots.contains_key(key))
    };
    let mut results = Vec::new();
    for link in links {
        let supply = (identity(&link.capacitor, &link.net), identity(&link.target, &link.net));
        let returned = identity(&link.return_pad, &link.return_net);
        let mut targets: Vec<PadReference> = board
            .nets
            .iter()
            .find(|net| net.name == link.return_net)
            .map(|net| net.pads.iter().filter(|p| p.component == link.target.component).cloned().collect())
            .unwrap_or_default();
        targets.sort();
        let target_ids: Vec<Option<String>> = targets.iter().map(|t| identity(t, &link.return_net)).collect();
        let (Some(first), Some(second), Some(returned)) = (&supply.0, &supply.1, &returned) else {
            results.push(assessment(link, "unsupported", None, None, Vec::new(), "ambiguous_or_missing_terminal"));
            continue;
        };
        if target_ids.iter().any(Option::is_none) {
            results.push(assessment(link, "unsupported", None, None, Vec::new(), "ambiguous_or_missing_terminal"));
            continue;
        }
        let supplied = roots[first] == roots[second];
        if targets.is_empty() {
            results.push(assessment(
                link,
                "unsupported",
                Some(supplied),
                None,
                Vec::new(),
                "no_declared_return_terminal_on_target",
            ));
            continue;
        }
        let joined: Vec<PadReference> = targets
            .iter()
            .zip(target_ids.iter())
            .filter(|(_, key)| roots[key.as_ref().expect("checked")] == roots[returned])
            .map(|(target, _)| target.clone())
            .collect();
        let connected = supplied && !joined.is_empty();
        let reason = if connected { "" } else { "supply_or_return_not_connected_by_supported_copper" };
        let returned_any = !joined.is_empty();
        results.push(assessment(
            link,
            if connected { "connected" } else { "open" },
            Some(supplied),
            Some(returned_any),
            joined,
            reason,
        ));
    }
    Ok(results)
}
