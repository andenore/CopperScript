//! Physical witnesses for declared decoupling (bypass) associations
//! (`pcbir.bypass_links`).

use indexmap::{IndexMap, IndexSet};

use copper_pcb::Result;
use copper_pcb::board::{DecouplingLink, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::transformed_pad_position;
use copper_pcb::zones::routing_layers;
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer};

use crate::decoupling_paths::{
    DecouplingCrossLayerWitness, refresh_crosslayer_witnesses, verify_crosslayer_decoupling,
};
use crate::pair_terminals::macro_owned_pads;
use crate::pin_escape::verified_land_path;
use crate::routing_clearance::RoutingClearanceIndex;

/// Per-link witness tracks or vias: original input identities, never synthesized ownership.
pub type WitnessPaths = IndexMap<DecouplingLink, Vec<TrackSegment>>;
pub type WitnessVias = IndexMap<DecouplingLink, Vec<Via>>;

/// `verified_bypass_links`: fresh physical witnesses for explicit source bypass associations.
///
/// Same-surface paths verify the feed only. Opposite-surface paths require both
/// supply and declared component-return joins through normal vias. No routing,
/// via shortcut or connected flag is used as evidence.
pub fn verified_bypass_links(
    board: &PhysicalBoard,
    only_nets: &IndexSet<String>,
    mut witness_paths: Option<&mut WitnessPaths>,
    mut witness_vias: Option<&mut WitnessVias>,
    admitted_witnesses: &[DecouplingCrossLayerWitness],
) -> Result<IndexSet<DecouplingLink>> {
    if let Some(paths) = witness_paths.as_deref_mut() {
        paths.clear();
    }
    if let Some(vias) = witness_vias.as_deref_mut() {
        vias.clear();
    }
    let poses = placement_map(board);
    let index = RoutingClearanceIndex::new(board)?;
    let owned = macro_owned_pads(board);
    let mut verified = IndexSet::new();
    for link in &board.decoupling_links {
        if !only_nets.contains(&link.net) || owned.contains(&link.target) || owned.contains(&link.capacitor) {
            continue;
        }
        let rule = board.net_routing_rules.iter().find(|r| r.net == link.net);
        if rule.is_some_and(|r| r.kind != RouteKind::General) {
            continue;
        }
        let (target, cap) = (&poses[&link.target.component], &poses[&link.capacitor.component]);
        let lands: Vec<_> = [(&link.target, target), (&link.capacitor, cap)]
            .into_iter()
            .flat_map(|(reference, pose)| {
                board.footprints[&pose.footprint].pads.iter().filter(move |p| p.number == reference.pad)
            })
            .collect();
        if lands.len() != 2 || lands.iter().any(|p| p.kind != PadKind::Smd) {
            continue;
        }
        if cap.side != target.side {
            let measured = verify_crosslayer_decoupling(board, link, None, 10_000, Some(&index))?;
            if let Some(witness) = measured.witness {
                verified.insert(link.clone());
                let paths = [&witness.supply_path, &witness.return_path];
                if let Some(witness_paths) = witness_paths.as_deref_mut() {
                    let tracks: IndexSet<TrackSegment> = paths.iter().flat_map(|p| p.tracks()).collect();
                    witness_paths.insert(link.clone(), tracks.into_iter().collect());
                }
                if let Some(witness_vias) = witness_vias.as_deref_mut() {
                    let vias: IndexSet<Via> = paths.iter().flat_map(|p| p.vias()).collect();
                    witness_vias.insert(link.clone(), vias.into_iter().collect());
                }
            }
            continue;
        }
        let layer = if target.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        if !routing_layers(board, &link.net, rule).contains(&layer) {
            continue;
        }
        let end = transformed_pad_position(board, cap, &link.capacitor.pad)?;
        let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
        let path =
            verified_land_path(board, lands[0], target, &link.net, end, &index, false, Some(layer), Some(width))?;
        if let Some(path) = path {
            verified.insert(link.clone());
            if let Some(witness_paths) = witness_paths.as_deref_mut() {
                witness_paths.insert(link.clone(), path);
            }
        }
    }
    if !admitted_witnesses.is_empty() {
        let (admitted, reason) =
            refresh_crosslayer_witnesses(board, admitted_witnesses, Some(&index), &IndexSet::new())?;
        if reason.is_some() {
            if let Some(paths) = witness_paths.as_deref_mut() {
                paths.clear();
            }
            if let Some(vias) = witness_vias.as_deref_mut() {
                vias.clear();
            }
            return Ok(IndexSet::new());
        }
        for witness in admitted {
            if !verified.contains(&witness.link) {
                continue;
            }
            let paths = [&witness.supply_path, &witness.return_path];
            if let Some(witness_paths) = witness_paths.as_deref_mut() {
                let mut tracks: IndexSet<TrackSegment> =
                    witness_paths.get(&witness.link).into_iter().flatten().cloned().collect();
                tracks.extend(paths.iter().flat_map(|p| p.tracks()));
                witness_paths.insert(witness.link.clone(), tracks.into_iter().collect());
            }
            if let Some(witness_vias) = witness_vias.as_deref_mut() {
                let mut vias: IndexSet<Via> = witness_vias.get(&witness.link).into_iter().flatten().cloned().collect();
                vias.extend(paths.iter().flat_map(|p| p.vias()));
                witness_vias.insert(witness.link.clone(), vias.into_iter().collect());
            }
        }
    }
    Ok(verified)
}
