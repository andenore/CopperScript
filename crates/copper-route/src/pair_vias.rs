//! Matched differential layer transitions for the joint pair search
//! (`pcbir.pair_vias`).
//!
//! Ported so far: the entry checks of `paired_via_candidates`, which decide
//! whether any transition layer is a target at all.

use indexmap::IndexSet;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard};
use copper_physical::CopperLayer;

use crate::Result;
use crate::pair_terminals::{terminal_copper_layers, terminal_partners};
use crate::routing::GlobalNetRoute;
use crate::routing_layers::routing_layers;

/// `paired_via_candidates` as far as it is ported: a pair without any
/// transition target layer yields nothing; transitions report not
/// implemented.
pub fn paired_via_candidates(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    first_guide: &GlobalNetRoute,
    second_guide: &GlobalNetRoute,
) -> Result<()> {
    let default = board.rules.default_track_width_nm;
    let width = first.width_nm.filter(|w| *w != 0).unwrap_or(default);
    if second.width_nm.filter(|w| *w != 0).unwrap_or(default) != width
        || first.pair_gap_nm.is_none()
        || second.pair_gap_nm != first.pair_gap_nm
        || first_guide.accesses.len() != 2
        || second_guide.accesses.len() != 2
    {
        return Ok(());
    }
    let Some(partners) = terminal_partners(first_guide, second_guide) else { return Ok(()) };
    let mut accesses: Vec<_> = first_guide.accesses.iter().collect();
    accesses.sort_by(|a, b| a.pad.component.cmp(&b.pad.component));
    let allowed: IndexSet<CopperLayer> = routing_layers(board, &first.net, Some(first))
        .into_iter()
        .filter(|layer| routing_layers(board, &second.net, Some(second)).contains(layer))
        .collect();
    let mut domains: Vec<IndexSet<CopperLayer>> = Vec::new();
    for access in &accesses {
        let partner = &partners[&format!("pad:{}", access.pad)];
        let own = terminal_copper_layers(board, &access.pad)?;
        let theirs = terminal_copper_layers(board, &partner.pad)?;
        domains.push(own.into_iter().filter(|layer| theirs.contains(layer) && allowed.contains(layer)).collect());
    }
    if domains.iter().any(IndexSet::is_empty) {
        return Ok(());
    }
    let order = &board.stackup.copper_layers;
    let common: Vec<CopperLayer> =
        allowed.iter().copied().filter(|layer| domains.iter().all(|domain| domain.contains(layer))).collect();
    let primary = common.iter().copied().min_by_key(|layer| crate::layer_index(order, *layer));
    let targets = allowed.iter().filter(|layer| Some(**layer) != primary).any(|layer| {
        let transitions = domains.iter().filter(|domain| !domain.contains(layer)).count() as i64;
        [first, second].iter().all(|rule| rule.max_vias.is_none_or(|limit| limit >= transitions))
    });
    if targets {
        // PENDING: _transitions, _transition_pairs and the transition searches.
        return copper_pcb::not_implemented("critical paired via transitions (pair_vias.paired_via_candidates)");
    }
    Ok(())
}
