//! One bounded late completion of original cross-surface bypass obligations
//! (`pcbir.fanout_bypass_completion`).
//!
//! Ordinary source launches remain ordinary owners. This transaction may append
//! semantic tails only; its rejection retains the complete incumbent fanout.
//! The completion report (`LateBypassCompletionReport`) reaches no output and
//! is not modelled.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{DecouplingLink, PadReference, PhysicalBoard};
use copper_physical::Point;

use crate::bypass_links::verified_bypass_links;
use crate::bypass_reservations::{BypassReservation, verified_bypass_reservations};
use crate::decoupling::{hard_findings, route_decouplers};
use crate::decoupling_paths::{budget_reason, normal_via, refresh_crosslayer_witnesses, track_ok};
use crate::fanout::{FanoutOptions, FanoutResult};
use crate::pin_escape::{RoutingAccess, verified_fanout_path, verified_routing_access};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::{Error, Result};

const OMITTED: [&str; 2] = ["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"];

/// `_ports_valid`: every held via port and surface handoff still verifies.
fn ports_valid(
    board: &PhysicalBoard,
    accesses: &IndexMap<PadReference, Point>,
    surfaces: Option<&IndexMap<PadReference, RoutingAccess>>,
    index: &RoutingClearanceIndex<'_>,
) -> Result<bool> {
    let names: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|net| net.pads.iter().map(move |pad| (pad, net.name.as_str()))).collect();
    for (pad, anchor) in accesses {
        let Some(net) = names.get(pad) else { return Ok(false) };
        if verified_fanout_path(board, pad, net, *anchor, index)?.is_none() {
            return Ok(false);
        }
    }
    for (pad, access) in surfaces.into_iter().flatten() {
        let Some(net) = names.get(pad) else { return Ok(false) };
        if verified_routing_access(board, pad, net, access, index)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether `items` has an element `others` lacks, counting multiplicity
/// (`Counter(items) - Counter(others)` is non-empty).
fn exceeds<T: Eq + std::hash::Hash>(items: &[T], others: &[T]) -> bool {
    let mut counts: IndexMap<&T, i64> = IndexMap::new();
    for item in items {
        *counts.entry(item).or_default() += 1;
    }
    for item in others {
        *counts.entry(item).or_default() -= 1;
    }
    counts.values().any(|n| *n > 0)
}

fn distinct<T: Eq + std::hash::Hash>(items: &[T]) -> bool {
    items.iter().collect::<IndexSet<_>>().len() == items.len()
}

/// `complete_late_bypasses`: one pass over exact early pending opposite-face
/// source links, no rounds.
pub fn complete_late_bypasses(
    incumbent: FanoutResult,
    original_pending: &[(DecouplingLink, String)],
    options: &FanoutOptions,
    only_nets: Option<&IndexSet<String>>,
) -> Result<FanoutResult> {
    let board = &incumbent.board;
    if original_pending.iter().any(|(link, _)| !board.decoupling_links.contains(link)) {
        return Err(Error::Value("late bypass selection contains an undeclared original obligation".to_owned()));
    }
    let side = |component: &str| board.placements.iter().find(|p| p.reference == component).map(|p| p.side);
    let selected: IndexSet<DecouplingLink> = original_pending
        .iter()
        .map(|(link, _)| link)
        .filter(|link| {
            side(&link.capacitor.component) != side(&link.target.component)
                && !options.plane_return_nets.contains(&link.net)
                && only_nets.is_none_or(|only| only.contains(&link.net))
        })
        .cloned()
        .collect();
    let mut incumbent = incumbent;
    // Every unchanged outcome keeps the original pending obligations.
    incumbent.decoupling_pending_links = original_pending.to_vec();
    if selected.is_empty() {
        return Ok(incumbent);
    }
    let board = &incumbent.board;
    let (created_tracks, created_vias) = (&incumbent.created_tracks, &incumbent.created_vias);
    if incumbent.added_track_count != created_tracks.len() as i64
        || incumbent.added_via_count != created_vias.len() as i64
        || board.tracks.len() < created_tracks.len()
        || board.vias.len() < created_vias.len()
        || board.tracks[board.tracks.len() - created_tracks.len()..] != created_tracks[..]
        || board.vias[board.vias.len() - created_vias.len()..] != created_vias[..]
    {
        return Ok(incumbent); // invalid_incumbent_generated_suffix
    }
    if !(exceeds(created_tracks, &incumbent.decoupling_tracks) || exceeds(created_vias, &incumbent.decoupling_vias)) {
        return Ok(incumbent); // no_new_ordinary_copper
    }
    let replacing = IndexSet::new();
    let index = RoutingClearanceIndex::new(board)?;
    let (old, reason) = refresh_crosslayer_witnesses(board, &incumbent.decoupling_witnesses, Some(&index), &replacing)?;
    if reason.is_some() || !ports_valid(board, &incumbent.accesses, incumbent.surface_accesses.as_ref(), &index)? {
        return Ok(incumbent);
    }
    drop(index);
    let nets: IndexSet<String> = board.decoupling_links.iter().map(|l| l.net.clone()).collect();
    let verified = verified_bypass_links(board, &nets, None, None, &old)?;
    let baseline = hard_findings(board, &OMITTED)?;
    let late = route_decouplers(board, only_nets, Some(options), Some(&selected), None)?;
    let candidate = &late.board;
    let mut intent = candidate.clone();
    intent.tracks = board.tracks.clone();
    intent.vias = board.vias.clone();
    if intent != *board
        || candidate.tracks.len() < board.tracks.len()
        || candidate.vias.len() < board.vias.len()
        || candidate.tracks[..board.tracks.len()] != board.tracks[..]
        || candidate.vias[..board.vias.len()] != board.vias[..]
    {
        return Ok(incumbent); // changed_incumbent_source_or_copper_prefix
    }
    let tracks = &candidate.tracks[board.tracks.len()..];
    let vias = &candidate.vias[board.vias.len()..];
    if tracks != late.created_tracks
        || vias != late.created_vias
        || tracks.iter().any(|t| board.tracks.contains(t))
        || vias.iter().any(|v| board.vias.contains(v))
        || !distinct(tracks)
        || !distinct(vias)
    {
        return Ok(incumbent); // ambiguous_late_generated_ownership
    }
    let fresh = RoutingClearanceIndex::new(candidate)?;
    let witnesses: Vec<_> = old.iter().chain(&late.admitted_path_witnesses).cloned().collect();
    let (admitted, reason) = refresh_crosslayer_witnesses(candidate, &witnesses, Some(&fresh), &replacing)?;
    if reason.is_some() {
        return Ok(incumbent);
    }
    let current = verified_bypass_links(candidate, &nets, None, None, &admitted)?;
    if !verified.iter().all(|link| current.contains(link))
        || candidate.nets.iter().any(|net| !budget_reason(candidate, &net.name).is_empty())
    {
        return Ok(incumbent);
    }
    for track in tracks {
        if !track_ok(candidate, &fresh, track, &track.net)? {
            return Ok(incumbent); // late_physical_gate_failed
        }
    }
    for via in vias {
        if !normal_via(candidate, &fresh, via)? {
            return Ok(incumbent);
        }
    }
    if !hard_findings(candidate, &OMITTED)?.iter().all(|f| baseline.contains(f))
        || !ports_valid(candidate, &incumbent.accesses, incumbent.surface_accesses.as_ref(), &fresh)?
    {
        return Ok(incumbent);
    }
    let completed: Vec<&DecouplingLink> = selected.iter().filter(|link| current.contains(*link)).collect();
    let mut permitted: IndexSet<&PadReference> =
        completed.iter().flat_map(|link| [&link.target, &link.capacitor, &link.return_pad]).collect();
    permitted.extend(admitted.iter().filter(|w| completed.contains(&&w.link)).map(|w| &w.return_target));
    if late.accesses.keys().any(|pad| !permitted.contains(pad)) {
        return Ok(incumbent); // unrelated_late_port
    }
    // Existing via, surface and boundary handoffs win unchanged.
    let held = |pad: &PadReference| {
        incumbent.accesses.contains_key(pad) || incumbent.surface_accesses.as_ref().is_some_and(|s| s.contains_key(pad))
    };
    let new: IndexMap<PadReference, Point> =
        late.accesses.iter().filter(|(pad, _)| !held(pad)).map(|(pad, anchor)| (pad.clone(), *anchor)).collect();
    if !ports_valid(candidate, &new, None, &fresh)? {
        return Ok(incumbent);
    }
    drop(fresh);
    let reasons: IndexMap<&DecouplingLink, &String> =
        late.pending.iter().map(|(link, reason)| (link, reason)).collect();
    let pending: Vec<(DecouplingLink, String)> = original_pending
        .iter()
        .filter(|(link, _)| !completed.contains(&link))
        .map(|(link, reason)| (link.clone(), reasons.get(link).map_or_else(|| reason.clone(), |r| (*r).clone())))
        .collect();
    let mut result = incumbent.clone();
    result.board = candidate.clone();
    result.created_tracks.extend_from_slice(tracks);
    result.created_vias.extend_from_slice(vias);
    result.added_track_count += tracks.len() as i64;
    result.added_via_count += vias.len() as i64;
    result.decoupling_tracks.extend_from_slice(tracks);
    result.decoupling_vias.extend_from_slice(vias);
    result.decoupling_witnesses = admitted;
    result.accesses.extend(new.iter().map(|(pad, anchor)| (pad.clone(), *anchor)));
    result.pending_pads.retain(|pad| !new.contains_key(pad));
    result.decoupling_pending_links = pending;
    // The final declared semantic ledger must be covered by actual complete paths.
    let mut source = board.clone();
    source.tracks.truncate(board.tracks.len() - incumbent.created_tracks.len());
    source.vias.truncate(board.vias.len() - incumbent.created_vias.len());
    let reserved = BypassReservation {
        board: result.board.clone(),
        created_tracks: result.created_tracks.clone(),
        created_vias: result.created_vias.clone(),
        added_track_count: result.added_track_count,
        added_via_count: result.added_via_count,
        decoupling_tracks: result.decoupling_tracks.clone(),
        decoupling_vias: result.decoupling_vias.clone(),
        decoupling_witnesses: result.decoupling_witnesses.clone(),
    };
    if verified_bypass_reservations(&source, &reserved)?.2.is_some() {
        return Ok(incumbent); // final_semantic_ownership_admission_failed
    }
    Ok(result)
}
