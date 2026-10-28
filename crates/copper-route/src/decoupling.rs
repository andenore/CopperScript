//! Pin-associated bypass placement and local surface routing preferences
//! (`pcbir.decoupling`).
//!
//! No capacitance, maximum distance, impedance or thermal limits are invented.
//! Fixed source poses, macro ownership, layer intent and clearances remain binding.

use indexmap::{IndexMap, IndexSet};

use copper_drc::{Policy, Severity};
use copper_pcb::board::{DecouplingLink, PadReference, PhysicalBoard, Placement, RouteKind, TrackSegment, Via};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::transformed_pad_position;
use copper_pcb::zones::routing_layers;
use copper_pcb::{Error, Result};
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_place::options::PlannerOptions;
use copper_place::scene::{Poses, Scene};

use crate::bypass_links::{WitnessPaths, WitnessVias, verified_bypass_links};
use crate::decoupling_paths::{
    DecouplingCrossLayerWitness, ProposalBounds, budget_reason, propose_crosslayer_decoupling,
    refresh_crosslayer_witnesses, verify_crosslayer_decoupling,
};
use crate::fanout::FanoutOptions;
use crate::pair_terminals::macro_owned_pads;
use crate::pin_escape::{checked_access_paths, verified_fanout_path, verified_land_path};
use crate::route_quality::escape_paths_cross;
use crate::routing_clearance::RoutingClearanceIndex;

/// A stage observer: `progress(stage)`.
pub type StageProgress<'a> = &'a mut dyn FnMut(&str);

/// A shorter reborrow of an optional observer, for a nested pass.
fn reborrow<'s>(progress: &'s mut Option<StageProgress<'_>>) -> Option<StageProgress<'s>> {
    progress.as_mut().map(|p| &mut **p as StageProgress<'s>)
}

/// `DecouplingRouteResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct DecouplingRouteResult {
    pub board: PhysicalBoard,
    pub created_tracks: Vec<TrackSegment>,
    pub connected_pads: IndexSet<PadReference>,
    pub pending: Vec<(DecouplingLink, String)>,
    pub created_vias: Vec<Via>,
    pub supply_return_witnesses: Vec<DecouplingCrossLayerWitness>,
    pub accesses: IndexMap<PadReference, Point>,
    /// Original complete paths at adoption, separately freshly bound at return.
    pub admitted_path_witnesses: Vec<DecouplingCrossLayerWitness>,
}

impl DecouplingRouteResult {
    fn unchanged(board: &PhysicalBoard) -> Self {
        DecouplingRouteResult {
            board: board.clone(),
            created_tracks: Vec::new(),
            connected_pads: IndexSet::new(),
            pending: Vec::new(),
            created_vias: Vec::new(),
            supply_return_witnesses: Vec::new(),
            accesses: IndexMap::new(),
            admitted_path_witnesses: Vec::new(),
        }
    }
}

fn surface_layer(side: BoardSide) -> CopperLayer {
    if side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }
}

fn with_copper(board: &PhysicalBoard, tracks: &[TrackSegment], vias: &[Via]) -> PhysicalBoard {
    let mut result = board.clone();
    result.tracks.extend_from_slice(tracks);
    result.vias.extend_from_slice(vias);
    result
}

/// The fingerprints (as their exact field tuple) of the hard DRC findings,
/// except the codes in `omitted`.
fn hard_findings(board: &PhysicalBoard, omitted: &[&str]) -> Result<IndexSet<String>> {
    let report = crate::drc_report::run_physical_drc(board, Policy::default())?;
    Ok(report
        .findings()
        .iter()
        .filter(|f| f.severity == Severity::Error && !omitted.contains(&f.code.as_str()))
        .map(|f| format!("{f:?}"))
        .collect())
}

/// `_surface_candidates`: cheap exact paths, including a straight lead beyond
/// the protected land.
///
/// A bend at the land center can hit an adjacent pin even when an outward
/// lead and the remaining elbow are clear. The lead derives from real land
/// dimensions and applicable clearance; this relaxes neither rule.
#[allow(clippy::too_many_arguments)]
fn surface_candidates(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    link: &DecouplingLink,
    target: &Placement,
    cap: &Placement,
    width: i64,
    layer: CopperLayer,
) -> Result<Vec<Vec<TrackSegment>>> {
    let start = transformed_pad_position(board, target, &link.target.pad)?;
    let end = transformed_pad_position(board, cap, &link.capacitor.pad)?;
    let mut result = checked_access_paths(board, index, &link.net, start, end, width, layer, false, true, None, false)?;
    let land = board.footprints[&target.footprint]
        .pads
        .iter()
        .find(|p| p.number == link.target.pad)
        .ok_or_else(|| Error::Value(format!("pad {} not found", link.target.pad)))?;
    let (nx, ny) = (start.x_nm - target.position.x_nm, start.y_nm - target.position.y_nm);
    if nx == 0 && ny == 0 {
        return Ok(result);
    }
    let (mut dx, mut dy) = (nx.signum(), ny.signum());
    if nx.abs() > 2 * ny.abs() {
        dy = 0;
    } else if ny.abs() > 2 * nx.abs() {
        dx = 0;
    }
    let rule = board.net_routing_rules.iter().find(|r| r.net == link.net);
    let clearance = board.rules.minimum_clearance_nm.max(rule.and_then(|r| r.clearance_nm).unwrap_or(0));
    let length = (land.size.width_nm.max(land.size.height_nm) + width + 1).div_euclid(2) + clearance;
    for multiplier in [1, 2] {
        let lead = Point::new(start.x_nm + dx * length * multiplier, start.y_nm + dy * length * multiplier);
        let prefix = checked_access_paths(board, index, &link.net, start, lead, width, layer, false, true, None, true)?;
        if let Some(prefix) = prefix.into_iter().next() {
            for tail in
                checked_access_paths(board, index, &link.net, lead, end, width, layer, false, true, None, false)?
            {
                result.push(prefix.iter().cloned().chain(tail).collect());
            }
        }
    }
    Ok(result)
}

fn path_length(path: &[TrackSegment]) -> f64 {
    path.iter().map(|t| ((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64)).sum()
}

/// `route_decouplers`: reserve explicit bypass feeds before ordinary package exits.
///
/// Existing same-surface supply paths retain their original semantics.
/// Opposite-surface parts require both actual supply and explicit component-
/// return paths, through permitted normal vias, before any copper is adopted.
/// Critical-profile nets remain under their specialized router's ownership.
/// Unsupported or blocked associations are returned explicitly, never hidden.
///
/// `only_links` is an exact subset of the original source links; it intersects
/// `only_nets`. Higher authored supply-net priority reserves its bypasses
/// first; equal priorities retain source declaration order.
pub fn route_decouplers(
    board: &PhysicalBoard,
    only_nets: Option<&IndexSet<String>>,
    fanout_options: Option<&FanoutOptions>,
    only_links: Option<&IndexSet<DecouplingLink>>,
    mut progress: Option<StageProgress<'_>>,
) -> Result<DecouplingRouteResult> {
    if let Some(only_links) = only_links {
        if !only_links.iter().all(|link| board.decoupling_links.contains(link)) {
            return Err(Error::Value("decoupling link selection contains an undeclared source association".to_owned()));
        }
        if only_links.is_empty() {
            return Ok(DecouplingRouteResult::unchanged(board));
        }
    }
    if board.decoupling_links.is_empty() {
        return Ok(DecouplingRouteResult::unchanged(board));
    }
    let poses = placement_map(board);
    let owned = macro_owned_pads(board);
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let mut index = RoutingClearanceIndex::new(board)?;
    let (mut tracks, mut vias): (Vec<TrackSegment>, Vec<Via>) = (Vec::new(), Vec::new());
    let mut successful: IndexSet<PadReference> = IndexSet::new();
    let mut pending: Vec<(DecouplingLink, String)> = Vec::new();
    let mut witnesses: Vec<(DecouplingLink, PadReference)> = Vec::new();
    let mut accesses: IndexMap<PadReference, Point> = IndexMap::new();
    let mut admitted_witnesses: Vec<DecouplingCrossLayerWitness> = Vec::new();
    let all_bypass_nets: IndexSet<String> = board.decoupling_links.iter().map(|l| l.net.clone()).collect();
    let mut verified = verified_bypass_links(board, &all_bypass_nets, None, None, &[])?;
    let admission = |candidate: &PhysicalBoard,
                     link: &DecouplingLink,
                     verified: &IndexSet<DecouplingLink>,
                     admitted: &[DecouplingCrossLayerWitness]|
     -> Result<(IndexSet<DecouplingLink>, &'static str)> {
        let reason = budget_reason(candidate, &link.net);
        if !reason.is_empty() {
            return Ok((IndexSet::new(), reason));
        }
        let current = verified_bypass_links(candidate, &all_bypass_nets, None, None, &[])?;
        if !(verified.iter().all(|l| current.contains(l)) && current.contains(link)) {
            return Ok((IndexSet::new(), "would_lose_verified_bypass_association"));
        }
        if !admitted.is_empty()
            && refresh_crosslayer_witnesses(candidate, admitted, None, &IndexSet::new())?.1.is_some()
        {
            return Ok((IndexSet::new(), "would_lose_admitted_bypass_path"));
        }
        Ok((current, ""))
    };
    let mut ordered_links: Vec<&DecouplingLink> = board.decoupling_links.iter().collect();
    ordered_links.sort_by_key(|link| -rules.get(link.net.as_str()).map_or(0, |r| r.priority));
    for link in ordered_links {
        if only_links.is_some_and(|links| !links.contains(link))
            || only_nets.is_some_and(|nets| !nets.contains(&link.net))
        {
            continue;
        }
        if let Some(progress) = progress.as_mut() {
            progress(&format!("decoupler_route:{}", link.capacitor.component));
        }
        if fanout_options.is_some_and(|o| o.plane_return_nets.contains(&link.net)) {
            pending.push((link.clone(), "native_plane_return_pending".to_owned()));
            continue;
        }
        if owned.contains(&link.target) || owned.contains(&link.capacitor) {
            continue; // Prescribed local copper is never regenerated.
        }
        let (target, cap) = (&poses[&link.target.component], &poses[&link.capacitor.component]);
        let rule = rules.get(link.net.as_str()).copied();
        if rule.is_some_and(|r| r.kind != RouteKind::General) {
            continue; // Explicit RF/critical topology wins, not generic bypass.
        }
        let layer = surface_layer(target.side);
        let lands: Vec<_> = [(&link.target, target), (&link.capacitor, cap)]
            .into_iter()
            .flat_map(|(reference, pose)| {
                board.footprints[&pose.footprint].pads.iter().filter(move |p| p.number == reference.pad)
            })
            .collect();
        if cap.side != target.side {
            let current = with_copper(board, &tracks, &vias);
            let measured =
                propose_crosslayer_decoupling(&current, link, fanout_options, &ProposalBounds::default(), &[], None)?;
            let Some(witness) = measured.witness.clone() else {
                pending.push((link.clone(), measured.reason));
                continue;
            };
            let (admitted, reason) = admission(&measured.board, link, &verified, &admitted_witnesses)?;
            if !reason.is_empty() {
                pending.push((link.clone(), reason.to_owned()));
                continue;
            }
            verified = admitted;
            witnesses.push((link.clone(), witness.return_target.clone()));
            successful.extend([link.target.clone(), link.capacitor.clone(), link.return_pad.clone()]);
            successful.insert(witness.return_target.clone());
            for track in &measured.created_tracks {
                tracks.push(track.clone());
                index.add_track(track.clone(), true, None)?;
            }
            for via in &measured.created_vias {
                vias.push(via.clone());
                index.add_via(via.clone(), true, None)?;
            }
            // Actual witnessed through-vias provide onward distribution ports.
            // They remain generated semantic owners, never immutable macros.
            let fresh = RoutingClearanceIndex::new(&measured.board)?;
            for path in [&witness.supply_path, &witness.return_path] {
                let forward = path.vias();
                let backward: Vec<Via> = forward.iter().rev().cloned().collect();
                for (endpoint, choices) in [(&path.start, &forward), (&path.end, &backward)] {
                    for via in choices {
                        if verified_fanout_path(&measured.board, endpoint, &path.net, via.position, &fresh)?.is_some() {
                            accesses.insert(endpoint.clone(), via.position);
                            break;
                        }
                    }
                }
            }
            admitted_witnesses.push(witness);
            continue;
        }
        if lands.len() != 2
            || lands.iter().any(|p| p.kind != PadKind::Smd)
            || !routing_layers(board, &link.net, rule).contains(&layer)
        {
            pending.push((link.clone(), "same-surface SMD path unavailable".to_owned()));
            continue;
        }
        let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
        let end = transformed_pad_position(board, cap, &link.capacitor.pad)?;
        // Preserve a verified multi-leg surface chain rather than adding a
        // parallel shortcut on a subsequent call. No via grants connectivity.
        let current_board = with_copper(board, &tracks, &vias);
        if verified_land_path(
            &current_board,
            lands[0],
            target,
            &link.net,
            end,
            &index,
            false,
            Some(layer),
            Some(width),
        )?
        .is_some()
        {
            let reason = budget_reason(&current_board, &link.net);
            if !reason.is_empty() {
                pending.push((link.clone(), reason.to_owned()));
                continue;
            }
            successful.extend([link.target.clone(), link.capacitor.clone()]);
            continue;
        }
        let mut candidates = surface_candidates(board, &index, link, target, cap, width, layer)?;
        candidates.sort_by(|a, b| path_length(a).total_cmp(&path_length(b)).then(a.len().cmp(&b.len())));
        let path = match candidates.into_iter().find(|p| !escape_paths_cross(p, &tracks)) {
            Some(path) => Some(path),
            None => {
                let start = transformed_pad_position(board, target, &link.target.pad)?;
                crate::surface_path::surface_path_between(
                    board,
                    &index,
                    &link.net,
                    start,
                    end,
                    width,
                    layer,
                    crate::nm_from_mm(1) / 10,
                    crate::nm_from_mm(1),
                    4000,
                )?
                .filter(|path| !escape_paths_cross(path, &tracks))
            }
        };
        let Some(path) = path else {
            pending.push((link.clone(), "bounded surface candidates blocked".to_owned()));
            continue;
        };
        let created: Vec<TrackSegment> =
            path.into_iter().filter(|t| !board.tracks.contains(t) && !tracks.contains(t)).collect();
        let mut candidate = with_copper(board, &tracks, &vias);
        candidate.tracks.extend(created.iter().cloned());
        let (admitted, reason) = admission(&candidate, link, &verified, &admitted_witnesses)?;
        if !reason.is_empty() {
            pending.push((link.clone(), reason.to_owned()));
            continue;
        }
        verified = admitted;
        successful.extend([link.target.clone(), link.capacitor.clone()]);
        for track in created {
            tracks.push(track.clone());
            index.add_track(track, true, None)?;
        }
    }
    drop(index);
    let result_board = with_copper(board, &tracks, &vias);
    let mut final_witnesses = Vec::new();
    let fresh_index = if witnesses.is_empty() && admitted_witnesses.is_empty() {
        None
    } else {
        Some(RoutingClearanceIndex::new(&result_board)?)
    };
    if let Some(fresh) = &fresh_index {
        for (link, target) in &witnesses {
            match verify_crosslayer_decoupling(&result_board, link, Some(target), 10_000, Some(fresh))?.witness {
                Some(witness) => final_witnesses.push(witness),
                None => return Err(Error::Value("decoupling batch lost an admitted complete association".to_owned())),
            }
        }
    }
    let (original_witnesses, reason) =
        refresh_crosslayer_witnesses(&result_board, &admitted_witnesses, fresh_index.as_ref(), &IndexSet::new())?;
    if let Some(reason) = reason {
        return Err(Error::Value(format!("decoupling batch lost an original admitted path: {reason}")));
    }
    drop(fresh_index);
    Ok(DecouplingRouteResult {
        board: result_board,
        created_tracks: tracks,
        connected_pads: successful,
        pending,
        created_vias: vias,
        supply_return_witnesses: final_witnesses,
        accesses,
        admitted_path_witnesses: original_witnesses,
    })
}

/// A successful pose trial: the reserved board, owned copper and fresh witnesses.
struct Trial {
    board: PhysicalBoard,
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    verified: IndexSet<DecouplingLink>,
    witnesses: WitnessPaths,
    via_witnesses: WitnessVias,
}

/// The ranking tuple of `improve_decoupling_placement`'s candidate poses.
type Rank = (bool, bool, i128, (bool, i128), i64, i64, copper_units::Decimal);

fn superset(larger: &IndexSet<DecouplingLink>, smaller: &IndexSet<DecouplingLink>) -> bool {
    smaller.iter().all(|link| larger.contains(link))
}

/// `improve_decoupling_placement`: bounded legal pad-relative relocation after
/// ordinary placement refinement.
///
/// Same side and a legal direct surface path win, then pad-centre distance.
/// Opposite-surface poses require fresh complete supply and component-return
/// joins. The search radius derives from the two footprints, not an electrical
/// maximum-distance requirement. Fixed, attached, macro and required-array
/// members are not moved. A candidate may replace only its own exclusive
/// generated feed; every other source-associated witness remains reserved.
/// `decoupling_placement_trials` limits copper-access trials per capacitor;
/// zero disables this pose heuristic.
///
/// `board` carries the refined poses (Python's `bypass_board`).
pub fn improve_decoupling_placement(
    board: &PhysicalBoard,
    placements: &Poses,
    options: &PlannerOptions,
    mut progress: Option<StageProgress<'_>>,
) -> Result<(Poses, i64)> {
    if !board.tracks.is_empty() || !board.vias.is_empty() {
        return Err(Error::Value("decoupling placement requires unrouted, unfilled source".to_owned()));
    }
    if board.decoupling_links.is_empty() {
        return Ok((placements.clone(), 0));
    }
    let scene = Scene::new(board, options.policy());
    let mut poses = placements.clone();
    let mut fixed: IndexSet<String> = scene.fixed_placements(&poses, &options.fixed_references).into_keys().collect();
    fixed.extend(board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.clone())));
    if board.via_in_pad_rules.iter().any(|r| r.rows.is_some()) {
        return copper_pcb::not_implemented("via-in-pad via arrays");
    }
    // Later-declared valid feeds are just as binding as earlier ones. Generate
    // this complete baseline once, then retain its exact generated prefix while
    // incrementally adding pending feeds. Successful labels alone are not proof.
    let frame = {
        let mut frame = board.clone();
        frame.placements = poses.values().cloned().collect();
        copper_drc::macros::materialize_hard_macros(frame)?
    };
    if options.decoupling_placement_trials == 0
        || board.decoupling_links.iter().all(|link| fixed.contains(&link.capacitor.component))
    {
        // This phase only proposes poses. With no movable capacitor, a full
        // disposable routing pass cannot change its result.
        return Ok((placements.clone(), 0));
    }
    if let Some(progress) = progress.as_mut() {
        progress("decoupler_baseline");
    }
    let reservation = route_decouplers(&frame, None, None, None, reborrow(&mut progress))?;
    let baseline_nets: IndexSet<String> = board.decoupling_links.iter().map(|l| l.net.clone()).collect();
    let (mut witnesses, mut via_witnesses) = (WitnessPaths::new(), WitnessVias::new());
    let mut verified =
        verified_bypass_links(&reservation.board, &baseline_nets, Some(&mut witnesses), Some(&mut via_witnesses), &[])?;
    let baseline_verified = verified.clone();
    let pending_links: IndexSet<&DecouplingLink> = reservation.pending.iter().map(|(link, _)| link).collect();
    let macro_owned = macro_owned_pads(&reservation.board);
    let mut owned_tracks = reservation.created_tracks.clone();
    let mut owned_vias = reservation.created_vias.clone();
    // Physical DRC uses default planner options for its placement finding.
    // This heuristic instead enforces the caller's explicit options below;
    // retain every copper/process finding without substituting that default.
    let omitted_hard = ["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE", "DRC-PLACEMENT"];
    let baseline_hard = hard_findings(&reservation.board, &omitted_hard)?;
    let mut reserved = reservation.board;
    let mut moved = 0;
    for link in &board.decoupling_links {
        let reference = &link.capacitor.component;
        if (!pending_links.contains(link) && !verified.contains(link))
            || fixed.contains(reference)
            || macro_owned.contains(&link.target)
            || macro_owned.contains(&link.capacitor)
        {
            continue;
        }
        if let Some(progress) = progress.as_mut() {
            progress(&format!("decoupler_refine:{reference}"));
        }
        let (current, target) = (poses[reference].clone(), poses[&link.target.component].clone());
        let point = transformed_pad_position(board, &target, &link.target.pad)?;
        let (cap_fp, target_fp) = (&board.footprints[&current.footprint], &board.footprints[&target.footprint]);
        // A small off-grid search spans local courtyard edges while allowing
        // orientation of the feed land toward the protected IC pad.
        let radius = target_fp.body_size.width_nm.max(target_fp.body_size.height_nm)
            + cap_fp.body_size.width_nm.max(cap_fp.body_size.height_nm)
            + scene.courtyard_gap(reference, &target.reference, options.component_clearance_nm)?;
        let step = 100_000.max(radius.div_euclid(24));
        let mut candidates = vec![current.clone()];
        let rule = board.placement_rules.iter().find(|r| &r.reference == reference);
        let sides: Vec<BoardSide> = match rule.and_then(|r| r.side) {
            Some(side) => vec![side],
            None => [target.side, current.side, BoardSide::Front, BoardSide::Back]
                .into_iter()
                .collect::<IndexSet<_>>()
                .into_iter()
                .collect(),
        };
        for &side in &sides {
            for angle in scene.allowed_orientations(reference) {
                let origin = Placement { position: Point::new(0, 0), rotation_degrees: angle, side, ..current.clone() };
                let offset = transformed_pad_position(board, &origin, &link.capacitor.pad)?;
                let mut distance = step;
                while distance <= radius {
                    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                        candidates.push(Placement {
                            position: Point::new(
                                point.x_nm + dx * distance - offset.x_nm,
                                point.y_nm + dy * distance - offset.y_nm,
                            ),
                            ..origin.clone()
                        });
                    }
                    distance += step;
                }
            }
        }
        let rank = |pose: &Placement| -> Result<Rank> {
            let terminal = transformed_pad_position(board, pose, &link.capacitor.pad)?;
            let (dx, dy) = (i128::from(terminal.x_nm - point.x_nm), i128::from(terminal.y_nm - point.y_nm));
            let (nx, ny) =
                (i128::from(point.x_nm - target.position.x_nm), i128::from(point.y_nm - target.position.y_nm));
            let outward = dx * nx + dy * ny;
            // Prefer the pin's outward half-cone over across-package placement.
            // This is a ranking hint: obstructed/central pins may still fall back.
            let outside = (nx != 0 || ny != 0)
                && (outward <= 0 || 2 * outward * outward < (dx * dx + dy * dy) * (nx * nx + ny * ny));
            // Among equally short feeds, point the supply end toward the IC
            // and the return end away. Tangential two-terminal bodies otherwise
            // win arbitrary XY/angle ties and occupy neighbouring pad exits.
            let mut end_on = (false, 0);
            if pose.side == target.side {
                let end = transformed_pad_position(board, pose, &link.return_pad.pad)?;
                let (ax, ay) = (i128::from(end.x_nm - terminal.x_nm), i128::from(end.y_nm - terminal.y_nm));
                end_on = (ax * dx + ay * dy <= 0, (ax * dy - ay * dx).abs());
            }
            Ok((
                pose.side != target.side,
                outside,
                dx * dx + dy * dy,
                end_on,
                pose.position.x_nm,
                pose.position.y_nm,
                pose.rotation_degrees.clone(),
            ))
        };
        let routing_rule = board.net_routing_rules.iter().find(|r| r.net == link.net);
        // All initially verified feeds and earlier pending feeds are physical
        // reservations. Source order cannot grant permission to lose a future
        // association that already had an exact valid witness.
        let own_path: IndexSet<&TrackSegment> = witnesses.get(link).into_iter().flatten().collect();
        let other_paths: IndexSet<&TrackSegment> =
            witnesses.iter().filter(|(other, _)| *other != link).flat_map(|(_, path)| path).collect();
        // Source/macro tracks never acquire generated ownership. Shared original
        // segments stay in the prefix even if the moving link also used them.
        let releasable: IndexSet<&TrackSegment> =
            own_path.into_iter().filter(|t| !other_paths.contains(t) && !frame.tracks.contains(t)).collect();
        let retained: Vec<TrackSegment> = owned_tracks.iter().filter(|t| !releasable.contains(t)).cloned().collect();
        let own_vias: IndexSet<&Via> = via_witnesses.get(link).into_iter().flatten().collect();
        let other_vias: IndexSet<&Via> =
            via_witnesses.iter().filter(|(other, _)| *other != link).flat_map(|(_, path)| path).collect();
        let releasable_vias: IndexSet<&Via> =
            own_vias.into_iter().filter(|v| !other_vias.contains(v) && !frame.vias.contains(v)).collect();
        let retained_vias: Vec<Via> = owned_vias.iter().filter(|v| !releasable_vias.contains(v)).cloned().collect();
        let direct_access = |candidate: &Placement, verified: &IndexSet<DecouplingLink>| -> Result<Option<Trial>> {
            let layer = surface_layer(target.side);
            if !routing_layers(board, &link.net, routing_rule).contains(&layer) {
                return Ok(None);
            }
            let mut scratch = reserved.clone();
            scratch.placements = board
                .placements
                .iter()
                .map(|p| if &p.reference == reference { candidate.clone() } else { poses[&p.reference].clone() })
                .collect();
            scratch.tracks = frame.tracks.iter().chain(&retained).cloned().collect();
            scratch.vias = frame.vias.iter().chain(&retained_vias).cloned().collect();
            let width =
                routing_rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
            let (trial, created, created_vias) = {
                let index = RoutingClearanceIndex::new(&scratch)?;
                // Moving this capacitor's return land must not cut any retained
                // bypass, even if its own feed path is individually clear.
                for t in &retained {
                    if !index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)? {
                        return Ok(None);
                    }
                }
                for v in &retained_vias {
                    if !index.can_via(
                        &v.net,
                        v.position,
                        v.size_nm,
                        v.from_layer,
                        v.to_layer,
                        Some(v.drill_nm),
                        false,
                        None,
                    )? {
                        return Ok(None);
                    }
                }
                if candidate.side != target.side {
                    let proposed =
                        propose_crosslayer_decoupling(&scratch, link, None, &ProposalBounds::default(), &[], None)?;
                    if proposed.witness.is_none() {
                        return Ok(None);
                    }
                    (proposed.board, proposed.created_tracks, proposed.created_vias)
                } else {
                    // The lazy generator's first path that crosses no existing same-net track.
                    let mut found = None;
                    for path in surface_candidates(&scratch, &index, link, &target, candidate, width, layer)? {
                        if !escape_paths_cross(&path, &scratch.tracks) {
                            found = Some(path);
                            break;
                        }
                    }
                    let Some(path) = found else {
                        return Ok(None);
                    };
                    let created: Vec<TrackSegment> = path.into_iter().filter(|t| !scratch.tracks.contains(t)).collect();
                    let mut trial = scratch.clone();
                    trial.tracks.extend(created.iter().cloned());
                    (trial, created, Vec::new())
                }
            };
            let (mut trial_witnesses, mut trial_via_witnesses) = (WitnessPaths::new(), WitnessVias::new());
            let trial_verified = verified_bypass_links(
                &trial,
                &baseline_nets,
                Some(&mut trial_witnesses),
                Some(&mut trial_via_witnesses),
                &[],
            )?;
            if !(superset(&trial_verified, verified) && trial_verified.contains(link)) {
                return Ok(None);
            }
            let trial_hard = hard_findings(&trial, &omitted_hard)?;
            if trial_hard.iter().any(|f| !baseline_hard.contains(f)) {
                return Ok(None);
            }
            Ok(Some(Trial {
                board: trial,
                tracks: retained.iter().cloned().chain(created).collect(),
                vias: retained_vias.iter().cloned().chain(created_vias).collect(),
                verified: trial_verified,
                witnesses: trial_witnesses,
                via_witnesses: trial_via_witnesses,
            }))
        };
        let current_access = direct_access(&current, &verified)?.is_some();
        let others: Poses =
            poses.iter().filter(|(r, _)| *r != reference).map(|(r, p)| (r.clone(), p.clone())).collect();
        let mut ranked = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            ranked.push((rank(&candidate)?, candidate));
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0));
        if !current_access && sides.len() > 1 {
            // An unrestricted pending association must not spend its entire
            // pose allowance on the target face. Interleave the existing
            // ranked domains; explicit side rules still have one domain.
            let mut domains: Vec<std::collections::VecDeque<(Rank, Placement)>> =
                sides.iter().map(|side| ranked.iter().filter(|(_, p)| p.side == *side).cloned().collect()).collect();
            let longest = domains.iter().map(|d| d.len()).max().unwrap_or(0);
            let mut interleaved = Vec::with_capacity(ranked.len());
            for _ in 0..longest {
                for domain in &mut domains {
                    interleaved.extend(domain.pop_front());
                }
            }
            ranked = interleaved;
        }
        let current_rank = rank(&current)?;
        let mut access_trials = 0;
        let mut adopted = None;
        for (candidate_rank, candidate) in ranked {
            if current_access && candidate_rank >= current_rank {
                break;
            }
            if !scene.legal(&candidate, &others)? {
                continue;
            }
            access_trials += 1;
            if let Some(progress) = progress.as_mut() {
                progress(&format!("decoupler_trial:{reference}:{access_trials}"));
            }
            let Some(trial) = direct_access(&candidate, &verified)? else {
                if access_trials >= options.decoupling_placement_trials {
                    break;
                }
                continue;
            };
            let mut proposed = poses.clone();
            proposed.insert(reference.clone(), candidate.clone());
            if !scene.checker.placement_solution_is_legal(&proposed)? {
                if access_trials >= options.decoupling_placement_trials {
                    break;
                }
                continue;
            }
            adopted = Some((candidate, trial));
            break;
        }
        if let Some((candidate, trial)) = adopted {
            moved += i64::from(candidate != current);
            poses.insert(reference.clone(), candidate);
            reserved = trial.board;
            owned_tracks = trial.tracks;
            owned_vias = trial.vias;
            verified = trial.verified;
            witnesses = trial.witnesses;
            via_witnesses = trial.via_witnesses;
        }
    }
    if moved != 0 {
        // The public output is unrouted poses, not a relabelled feed macro.
        // Fresh normal regeneration must preserve identities as well as the
        // internal proposal ledger; otherwise roll back this bounded heuristic.
        if let Some(progress) = progress.as_mut() {
            progress("decoupler_regenerate");
        }
        let mut source = board.clone();
        source.placements = poses.values().cloned().collect();
        let regenerated = route_decouplers(
            &copper_drc::macros::materialize_hard_macros(source)?,
            None,
            None,
            None,
            reborrow(&mut progress),
        )?;
        let now = verified_bypass_links(&regenerated.board, &baseline_nets, None, None, &[])?;
        if !(superset(&now, &baseline_verified) && superset(&now, &verified)) {
            return Ok((placements.clone(), 0));
        }
        let regenerated_hard = hard_findings(&regenerated.board, &omitted_hard)?;
        if regenerated_hard.iter().any(|f| !baseline_hard.contains(f)) {
            return Ok((placements.clone(), 0));
        }
    }
    Ok((poses, moved))
}
