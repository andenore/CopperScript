//! Bounded rigid-cluster legalization and whole-cluster refinement
//! (`pcbir.cluster_placement`). Ordinary components never move a member alone.

use std::cmp::Reverse;
use std::collections::VecDeque;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{Placement, RelativePlacementKind, RigidPlacementCluster};
use copper_pcb::hard_macros::cluster_placements;
use copper_pcb::keepouts::resolved_cluster_keepouts;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::{Error, Result};
use copper_physical::Point;
use copper_units::Decimal;

use crate::escape::{EscapeSpacingModel, placement_units};
use crate::legalize::{
    LegalizationFailure, PackingDiagnostics, adjacency, best_legal_choice, candidate_positions, pack_local_components,
    pack_local_components_observed, relative_cluster_order, spread_index,
};
use crate::metrics::{fast_score, hpwl, placement_metrics};
use crate::options::PlannerOptions;
use crate::refine::nearby_positions;
use crate::scene::{Poses, Scene, point_bounds, posed, with};

fn error<T>(message: String) -> Result<T> {
    Err(Error::Value(message))
}

/// `((value % 360) + 360) % 360`.
fn positive_angle(value: &Decimal) -> Result<Decimal> {
    copper_pcb::board::positive_rem(value)
}

/// `place_rigid_clusters`: legalize each cluster as a whole before ordinary components.
///
/// Returns the updated targets and source poses, and the phase options that
/// lock every cluster member.
pub fn place_rigid_clusters(
    scene: &Scene,
    targets: &Poses,
    original: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<(Poses, Poses, PlannerOptions)> {
    match try_place_rigid_clusters(scene, targets, original, options, spacing)? {
        ClusterPlacement::Placed(targets, original, phase, _) => Ok((targets, original, phase)),
        ClusterPlacement::Failed(failure) => error(failure.message),
    }
}

/// A bounded mutable search may discard a seed; invalid fixed geometry remains an error.
pub(crate) enum ClusterPlacement {
    Placed(Poses, Poses, PlannerOptions, Poses),
    Failed(LegalizationFailure),
}

pub(crate) fn try_place_rigid_clusters(
    scene: &Scene,
    targets: &Poses,
    original: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<ClusterPlacement> {
    let board = scene.board;
    let (mut targets, mut original) = (targets.clone(), original.clone());
    let fixed = scene.fixed_placements(&original, &options.fixed_references);
    let members: IndexSet<String> =
        board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.clone())).collect();
    let mut regional_domains: Vec<(usize, String)> = targets
        .iter()
        .filter(|(reference, _)| {
            !fixed.contains_key(*reference)
                && !members.contains(*reference)
                && scene.rule(reference).is_some_and(|rule| rule.region.is_some())
                && !scene.relative.iter().any(|rule| rule.targets.iter().any(|target| target.reference == **reference))
        })
        .map(|(reference, target)| {
            let count = candidate_positions(scene, target, options)
                .len()
                .saturating_mul(scene.allowed_orientations(reference).len());
            (count, reference.clone())
        })
        .collect();
    regional_domains.sort();
    let (regions, broad_regions) = regional_reservation_groups(scene, &regional_domains, options)?;
    let mut reservations = Poses::new();
    // Unrelated fixed components are obstacles, not movable cluster companions.
    let mut placed: Poses =
        fixed.iter().filter(|(r, _)| !members.contains(*r)).map(|(r, p)| (r.clone(), p.clone())).collect();
    let mut obstacles: Vec<(&String, &Placement)> = placed.iter().collect();
    obstacles.sort_by(|a, b| a.0.cmp(b.0));
    for (index, (reference, pose)) in obstacles.iter().enumerate() {
        for (other_reference, other) in &obstacles[..index] {
            if pose.side == other.side
                && copper_drc::legality::courtyards_too_close(
                    &scene.polygon(pose),
                    &scene.polygon(other),
                    scene.courtyard_gap(reference, other_reference, options.component_clearance_nm)?,
                )
            {
                return error(format!(
                    "conflicting fixed placements {} and {}: courtyard clearance; cluster search cannot relocate either",
                    copper_json::python_str_repr(other_reference),
                    copper_json::python_str_repr(reference)
                ));
            }
        }
    }
    let mut clusters: Vec<&RigidPlacementCluster> = board.rigid_clusters.iter().collect();
    clusters.sort_by_key(|c| {
        // Reserve immutable geometry first, then named mechanical regions,
        // before unrestricted groups can consume their limited space.
        let class = if c.members.iter().any(|m| fixed.contains_key(&m.reference)) {
            0
        } else if c.members.iter().any(|m| scene.rule(&m.reference).is_some_and(|rule| rule.region.is_some())) {
            1
        } else {
            2
        };
        (class, c.name.clone())
    });
    let mut reserved = false;
    let mut broad_reserved = false;
    for cluster in clusters {
        if !reserved && !cluster.members.iter().any(|m| fixed.contains_key(&m.reference)) {
            if let Some(failure) =
                reserve_regions(scene, &regions, &mut targets, &mut placed, &mut reservations, options, spacing)?
            {
                return Ok(ClusterPlacement::Failed(failure));
            }
            let units = placement_units(board, &scene.relative);
            let mut companions: Vec<String> = targets
                .keys()
                .filter(|r| {
                    !members.contains(*r)
                        && !placed.contains_key(*r)
                        && units[*r].iter().any(|u| placed.contains_key(u))
                })
                .cloned()
                .collect();
            companions.sort();
            let adjacency = adjacency(scene);
            let ordered = relative_cluster_order(scene, &companions, &targets);
            // Reserve constrained companions jointly before unrelated macros consume their space.
            let mut seen: Vec<*const std::collections::BTreeSet<String>> = Vec::new();
            for reference in &ordered {
                let unit = &units[reference];
                if seen.contains(&std::rc::Rc::as_ptr(unit)) {
                    continue;
                }
                seen.push(std::rc::Rc::as_ptr(unit));
                let references: Vec<String> = ordered.iter().filter(|r| unit.contains(*r)).cloned().collect();
                if references.len() < 2 {
                    continue;
                }
                if let Some(packed) =
                    pack_local_components(scene, &references, &placed, &targets, options, true, false, &[])?
                {
                    for reference in &references {
                        targets.insert(reference.clone(), packed[reference].clone());
                    }
                    placed = packed;
                }
            }
            for reference in &ordered {
                if placed.contains_key(reference) {
                    continue;
                }
                let choice = best_legal_choice(
                    scene,
                    reference,
                    &targets[reference],
                    &placed,
                    &adjacency,
                    options,
                    spacing,
                    0,
                    Some(options.legalization_candidates as usize),
                )?;
                let accepted = match &choice {
                    None => false,
                    Some(pose) => scene.penalty(&with(&placed, pose))? <= scene.penalty(&placed)?,
                };
                if !accepted {
                    return Ok(ClusterPlacement::Failed(LegalizationFailure {
                        message: format!(
                            "cannot reserve fixed-anchor companion {} before movable rigid clusters",
                            copper_json::python_str_repr(reference)
                        ),
                        failed_reference: reference.clone(),
                        accepted: placed,
                        bounded_packing: None,
                    }));
                }
                let choice = choice.expect("a choice");
                placed.insert(reference.clone(), choice.clone());
                targets.insert(reference.clone(), choice);
            }
            reserved = true;
        }
        if !broad_reserved
            && !cluster.members.iter().any(|member| {
                fixed.contains_key(&member.reference)
                    || scene.rule(&member.reference).is_some_and(|rule| rule.region.is_some())
            })
        {
            if let Some(failure) = reserve_regional_components(
                scene,
                &broad_regions,
                &mut targets,
                &mut placed,
                &mut reservations,
                options,
            )? {
                return Ok(ClusterPlacement::Failed(failure));
            }
            broad_reserved = true;
        }
        let local_anchor = cluster.members.iter().find(|m| m.reference == cluster.anchor_reference).expect("anchor");
        let mut target = targets[&cluster.anchor_reference].clone();
        let fixed_members: Vec<&copper_pcb::board::RigidPlacementMember> =
            cluster.members.iter().filter(|m| fixed.contains_key(&m.reference)).collect();
        let (companions, deferred_companions) = if fixed_members.is_empty() {
            movable_cluster_companions(scene, cluster, &members, &fixed, &placed, &targets, options)
        } else {
            (Vec::new(), 0)
        };
        if deferred_companions > 0 {
            scene.progress(&format!("cluster_companions_deferred:{}:{deferred_companions}", cluster.name));
        }
        let mut witness_trials = 0;
        let mut legal_macro_poses = 0;
        let reservation_rules: Vec<_> = scene
            .relative
            .iter()
            .copied()
            .filter(|rule| rule.targets.iter().any(|target| reservations.contains_key(&target.reference)))
            .collect();
        let mut remaining_positions = Vec::new();
        let mut poses: Vec<Placement> = if let Some(member) = fixed_members.first() {
            let known = &fixed[&member.reference];
            let rotation = positive_angle(
                &known.rotation_degrees.sub(&member.rotation_degrees).map_err(|e| Error::Value(e.to_string()))?,
            )?;
            let frame = Placement { rotation_degrees: rotation.clone(), ..known.clone() };
            let offset = Point::new(
                local_anchor.position.x_nm - member.position.x_nm,
                local_anchor.position.y_nm - member.position.y_nm,
            );
            target = posed(&original[&cluster.anchor_reference], transformed_local_point(&frame, offset), &rotation);
            vec![target.clone()]
        } else {
            let mut positions: IndexSet<Point> = IndexSet::new();
            positions.insert(target.position);
            positions.extend(candidate_positions(scene, &target, options));
            remaining_positions = positions.iter().skip(options.legalization_candidates as usize).copied().collect();
            positions
                .into_iter()
                .take(options.legalization_candidates as usize)
                .flat_map(|point| cluster.allowed_rotations.iter().map(move |angle| (point, angle.clone())))
                .map(|(point, angle)| posed(&target, point, &angle))
                .collect()
        };
        let mut best: Option<((i64, i64, i64, i64), Decimal, IndexMap<String, Placement>)> = None;
        let mut probes = 0;
        for pass in 0..2 {
            if pass == 1 {
                // Preserve every successful nearest-first choice. Only an exhausted
                // mutable cluster spends the residual raw budget across its domain.
                if best.is_some() || !fixed_members.is_empty() {
                    break;
                }
                let remaining = options.probe_limit().saturating_sub(probes);
                let count = remaining.div_ceil(cluster.allowed_rotations.len()).min(remaining_positions.len());
                if count == 0 {
                    break;
                }
                scene.progress(&format!("cluster_wide:{}", cluster.name));
                poses = (0..count)
                    .map(|index| remaining_positions[spread_index(index, remaining_positions.len(), count)])
                    .flat_map(|point| cluster.allowed_rotations.iter().map(move |angle| (point, angle)))
                    .take(remaining)
                    .map(|(point, angle)| posed(&target, point, angle))
                    .collect();
            }
            let mut witness_candidates = Vec::new();
            for pose in &poses {
                if probes == options.probe_limit() {
                    break;
                }
                // Count even rejected transforms and illegal geometry as a probe.
                probes += 1;
                let Ok(placed_members) = cluster_placements(cluster, pose, &original) else { continue };
                if fixed_members.iter().any(|m| placed_members[&m.reference] != fixed[&m.reference]) {
                    continue;
                }
                let mut trial = placed.clone();
                for (reference, member) in &placed_members {
                    trial.insert(reference.clone(), member.clone());
                }
                let keepouts = resolved_cluster_keepouts(board, &trial);
                let mut legal = true;
                // Newly moved members usually expose a blocked trial first. Held
                // components still need checking against the translated keepouts.
                for item in placed_members
                    .values()
                    .chain(trial.values().filter(|item| !placed_members.contains_key(&item.reference)))
                {
                    let others = crate::scene::without(&trial, &item.reference);
                    if !scene.checker.legal(item, &others, Some(&keepouts))? {
                        legal = false;
                        break;
                    }
                }
                if !legal {
                    continue;
                }
                if scene.relative_penalty(&trial, &reservation_rules)? != 0 {
                    continue;
                }
                let mut merged = targets.clone();
                for (reference, member) in &trial {
                    merged.insert(reference.clone(), member.clone());
                }
                let rank = (
                    hpwl(scene, &merged)?,
                    (pose.position.x_nm - target.position.x_nm).abs()
                        + (pose.position.y_nm - target.position.y_nm).abs(),
                    pose.position.y_nm,
                    pose.position.x_nm,
                );
                if !companions.is_empty() {
                    legal_macro_poses += 1;
                    witness_candidates.push((rank, pose.clone()));
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some((key, angle, _)) => (&rank, &pose.rotation_degrees) < (key, angle),
                };
                if better {
                    best = Some((rank, pose.rotation_degrees.clone(), placed_members));
                }
            }
            if !witness_candidates.is_empty() {
                witness_candidates.sort_by(|a, b| (&a.0, &a.1.rotation_degrees).cmp(&(&b.0, &b.1.rotation_degrees)));
                let limit = (options.exact_repair_candidates as usize).max(1).min(witness_candidates.len());
                // Keep the best original choices, but do not spend every trial
                // on neighboring poses of the same blocked pocket. The rest
                // span the complete legal, ranked domain of this macro pass.
                let near = limit.div_ceil(2);
                let indices = (0..near).chain((0..limit - near).map(|i| {
                    if limit - near == 1 {
                        witness_candidates.len() - 1
                    } else {
                        near + spread_index(i, witness_candidates.len() - near, limit - near)
                    }
                }));
                for index in indices {
                    witness_trials += 1;
                    scene.progress(&format!("cluster_witness:{}:{witness_trials}", cluster.name));
                    let (rank, pose) = &witness_candidates[index];
                    let placed_members = cluster_placements(cluster, pose, &original)?;
                    let mut trial = placed.clone();
                    trial.extend(placed_members.clone());
                    let mut trial_targets = targets.clone();
                    trial_targets.extend(trial.clone());
                    let Some(packed) =
                        pack_local_components(scene, &companions, &trial, &trial_targets, options, true, false, &[])?
                    else {
                        continue;
                    };
                    // The packer checks the complete active companion rules,
                    // exact macro copper/keepouts, side and courtyard geometry.
                    // Retain only this macro and its new ordinary companions;
                    // failed trials have never changed the held placements.
                    let mut chosen = placed_members;
                    for reference in &companions {
                        chosen.insert(reference.clone(), packed[reference].clone());
                    }
                    best = Some((*rank, pose.rotation_degrees.clone(), chosen));
                    break;
                }
            }
        }
        let Some((_, _, chosen)) = best else {
            if !fixed_members.is_empty() {
                return error(format!(
                    "fixed rigid cluster {} violates its fixed poses, board, keepout, region, or clearance constraints",
                    copper_json::python_str_repr(&cluster.name)
                ));
            }
            return Ok(ClusterPlacement::Failed(LegalizationFailure {
                message: if companions.is_empty() {
                    format!(
                        "cannot legalize rigid cluster {} within its candidate budget",
                        copper_json::python_str_repr(&cluster.name)
                    )
                } else {
                    format!(
                        "cannot legalize rigid cluster {} with ordinary companions {}: no sampled joint placement witness in {witness_trials} bounded trials from {legal_macro_poses} legal macro poses; this is not a proof of geometric impossibility",
                        copper_json::python_str_repr(&cluster.name),
                        companions.join(", ")
                    )
                },
                failed_reference: cluster.anchor_reference.clone(),
                accepted: placed,
                bounded_packing: None,
            }));
        };
        for (reference, pose) in chosen {
            if !members.contains(&reference) {
                reservations.insert(reference.clone(), pose.clone());
            }
            placed.insert(reference.clone(), pose.clone());
            original.insert(reference.clone(), pose.clone());
            targets.insert(reference, pose);
        }
    }
    if !reserved {
        if let Some(failure) =
            reserve_regions(scene, &regions, &mut targets, &mut placed, &mut reservations, options, spacing)?
        {
            return Ok(ClusterPlacement::Failed(failure));
        }
    }
    if !broad_reserved {
        if let Some(failure) =
            reserve_regional_components(scene, &broad_regions, &mut targets, &mut placed, &mut reservations, options)?
        {
            return Ok(ClusterPlacement::Failed(failure));
        }
    }
    let mut phase = options.clone();
    phase.fixed_references.extend(members);
    Ok(ClusterPlacement::Placed(targets, original, phase, reservations))
}

/// Follow authored pin-local maximum-distance links through ordinary parts.
/// Other rigid groups and source-fixed owners are boundaries, never companions
/// to translate or unlock. An accepted witness remains a mutable reservation.
fn movable_cluster_companions(
    scene: &Scene,
    cluster: &RigidPlacementCluster,
    rigid: &IndexSet<String>,
    fixed: &Poses,
    placed: &Poses,
    targets: &Poses,
    options: &PlannerOptions,
) -> (Vec<String>, usize) {
    let mut reached: IndexMap<String, usize> =
        cluster.members.iter().map(|member| (member.reference.clone(), 0)).collect();
    let mut queue: VecDeque<String> = reached.keys().cloned().collect();
    while let Some(reference) = queue.pop_front() {
        let hops = reached[&reference];
        for rule in &scene.relative {
            if rule.kind != RelativePlacementKind::MaxDistance
                || rule.targets.len() != 2
                || rule.targets.iter().any(|target| target.pad.is_none())
                || !rule.targets.iter().any(|target| target.reference == reference)
            {
                continue;
            }
            for target in &rule.targets {
                if !rigid.contains(&target.reference)
                    && !fixed.contains_key(&target.reference)
                    && !reached.contains_key(&target.reference)
                {
                    reached.insert(target.reference.clone(), hops + 1);
                    queue.push_back(target.reference.clone());
                }
            }
        }
    }
    let mut companions: Vec<String> = reached
        .keys()
        .filter(|reference| !rigid.contains(*reference) && !placed.contains_key(*reference))
        .cloned()
        .collect();
    companions.sort_by_key(|reference| {
        (reached[reference], Reverse(scene.footprint_area(&targets[reference])), reference.clone())
    });
    let limit = (options.exact_repair_limit as usize).max(1);
    let deferred = companions.len().saturating_sub(limit);
    companions.truncate(limit);
    // Earlier hops precede later hops, keeping selected dependencies connected.
    // Omitted members retain ordinary placement and all complete final gates.
    (companions, deferred)
}

/// Regional rigid geometry gets first claim before these broader mechanical
/// landings. Pack each bounded set jointly so overlapping regions do not become
/// independent greedy claims. Reservations remain mutable during legalization.
fn reserve_regional_components(
    scene: &Scene,
    references: &[String],
    targets: &mut Poses,
    placed: &mut Poses,
    reservations: &mut Poses,
    options: &PlannerOptions,
) -> Result<Option<LegalizationFailure>> {
    for batch in references.chunks(options.exact_repair_limit.max(1) as usize) {
        scene.progress(&format!("region_pack:{}", batch.join(",")));
        let mut diagnostic = PackingDiagnostics::default();
        let Some(packed) = pack_local_components_observed(scene, batch, placed, targets, options, &mut diagnostic)?
        else {
            let domains = diagnostic
                .domains
                .iter()
                .map(|(reference, (probes, legal))| {
                    format!("{reference}: {legal} individually legal sampled poses / {probes} raw probes")
                })
                .collect::<Vec<_>>()
                .join("; ");
            let cause = if let Some(reference) = &diagnostic.empty_reference {
                format!("empty sampled domain for {reference}")
            } else if diagnostic.search_remaining == Some(0) {
                "joint search budget exhausted".to_owned()
            } else {
                format!(
                    "no compatible sampled combination; {} joint search steps remain",
                    diagnostic.search_remaining.unwrap_or(0)
                )
            };
            return Ok(Some(LegalizationFailure {
                message: format!(
                    "cannot jointly reserve regional components {} within the existing packing budget: {cause}; {domains}",
                    batch.join(", "),
                ),
                failed_reference: diagnostic.empty_reference.unwrap_or_else(|| batch[0].clone()),
                accepted: placed.clone(),
                bounded_packing: None,
            }));
        };
        for reference in batch {
            let pose = packed[reference].clone();
            targets.insert(reference.clone(), pose.clone());
            placed.insert(reference.clone(), pose.clone());
            reservations.insert(reference.clone(), pose);
        }
    }
    Ok(None)
}

/// A small sampled domain is scarce, but it is not an immutable reservation.
/// When its authored region interacts with a broad regional domain, search them
/// together after named-region rigid geometry. Otherwise a greedy narrow pose
/// can erase every broad pose before the joint packer sees the narrow member.
/// Unrelated scarce singletons keep their existing early scheduling. Region
/// bounds are a conservative grouping aid, never a replacement for exact gates;
/// oversized groups retain the existing bounded per-batch search limit.
fn regional_reservation_groups(
    scene: &Scene,
    domains: &[(usize, String)],
    options: &PlannerOptions,
) -> Result<(Vec<String>, Vec<String>)> {
    let bounds: Vec<_> = domains
        .iter()
        .map(|(_, reference)| {
            let name = scene.rule(reference).and_then(|rule| rule.region.as_ref()).expect("regional member");
            let region = scene.board.regions.iter().find(|region| &region.name == name).expect("resolved region");
            point_bounds(&region.outline.vertices)
        })
        .collect();
    let mut neighbors = vec![Vec::new(); domains.len()];
    for a in 0..domains.len() {
        for b in a + 1..domains.len() {
            let gap = scene.courtyard_gap(&domains[a].1, &domains[b].1, options.component_clearance_nm)?;
            let (ax0, ay0, ax1, ay1) = bounds[a];
            let (bx0, by0, bx1, by1) = bounds[b];
            if ax0 <= bx1.saturating_add(gap)
                && bx0 <= ax1.saturating_add(gap)
                && ay0 <= by1.saturating_add(gap)
                && by0 <= ay1.saturating_add(gap)
            {
                neighbors[a].push(b);
                neighbors[b].push(a);
            }
        }
    }
    let mut seen = vec![false; domains.len()];
    let mut joint = vec![false; domains.len()];
    for start in 0..domains.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut group = vec![start];
        let mut next = 0;
        while next < group.len() {
            for &neighbor in &neighbors[group[next]] {
                if !seen[neighbor] {
                    seen[neighbor] = true;
                    group.push(neighbor);
                }
            }
            next += 1;
        }
        group.sort_unstable();
        if group.iter().any(|&index| domains[index].0 > options.legalization_candidates as usize) {
            for index in group {
                joint[index] = true;
            }
        }
    }
    let mut narrow = Vec::new();
    let mut broad = Vec::new();
    for (index, (_, reference)) in domains.iter().enumerate() {
        if joint[index] {
            broad.push(reference.clone());
        } else {
            narrow.push(reference.clone());
        }
    }
    // Preserve the original broad batching: assembly-access envelopes can
    // interact beyond authored region bounds. Do not split disjoint regions
    // into independent searches; only promote the interacting narrow members.
    Ok((narrow, broad))
}

/// Schedule scarce region-only components once, before unrestricted groups.
/// These poses remain mutable; only the actual rigid members enter phase locks.
fn reserve_regions(
    scene: &Scene,
    references: &[String],
    targets: &mut Poses,
    placed: &mut Poses,
    reservations: &mut Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<Option<LegalizationFailure>> {
    let adjacency = adjacency(scene);
    for reference in references {
        scene.progress(&format!("region_reserve:{reference}"));
        let choice = best_legal_choice(
            scene,
            reference,
            &targets[reference],
            placed,
            &adjacency,
            options,
            spacing,
            0,
            Some(options.legalization_candidates as usize),
        )?;
        let Some(pose) = choice else {
            return Ok(Some(LegalizationFailure {
                message: format!(
                    "cannot place narrow-region component {} within its candidate budget",
                    copper_json::python_str_repr(reference)
                ),
                failed_reference: reference.clone(),
                accepted: placed.clone(),
                bounded_packing: None,
            }));
        };
        targets.insert(reference.clone(), pose.clone());
        placed.insert(reference.clone(), pose.clone());
        reservations.insert(reference.clone(), pose);
    }
    Ok(None)
}

/// `refine_rigid_clusters`: move whole clusters among nearby anchor poses when the fast score improves.
pub fn refine_rigid_clusters(
    scene: &Scene,
    source: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<(Poses, i64)> {
    let board = scene.board;
    let mut placements = source.clone();
    let fixed: IndexSet<String> = scene.fixed_placements(source, &options.fixed_references).into_keys().collect();
    let mut moves = 0;
    let mut clusters: Vec<&RigidPlacementCluster> = board.rigid_clusters.iter().collect();
    clusters.sort_by(|a, b| a.name.cmp(&b.name));
    for _ in 0..options.refinement_passes {
        let mut changed = false;
        for cluster in &clusters {
            if cluster.members.iter().any(|m| fixed.contains(&m.reference)) {
                continue;
            }
            let anchor = placements[&cluster.anchor_reference].clone();
            let mut best: Option<Poses> = None;
            let mut best_rank = (
                fast_score(scene, &placements, spacing, options.power_domain_weight)?,
                anchor.position.y_nm,
                anchor.position.x_nm,
                anchor.rotation_degrees.clone(),
            );
            let baseline = placement_metrics(scene, &placements, options, spacing)?;
            let mut points = vec![anchor.position];
            points.extend(nearby_positions(anchor.position, options));
            for point in points {
                for angle in &cluster.allowed_rotations {
                    let pose = posed(&anchor, point, angle);
                    let mut trial = placements.clone();
                    for (reference, member) in cluster_placements(cluster, &pose, &placements)? {
                        trial.insert(reference, member);
                    }
                    if !scene.checker.placement_solution_is_legal(&trial)? {
                        continue;
                    }
                    let rank = (
                        fast_score(scene, &trial, spacing, options.power_domain_weight)?,
                        point.y_nm,
                        point.x_nm,
                        angle.clone(),
                    );
                    if rank >= best_rank {
                        continue;
                    }
                    let metrics = placement_metrics(scene, &trial, options, spacing)?;
                    if metrics.constraint_penalty_nm > baseline.constraint_penalty_nm
                        || metrics.congestion_overflow > baseline.congestion_overflow
                    {
                        continue;
                    }
                    best = Some(trial);
                    best_rank = rank;
                }
            }
            if let Some(best) = best {
                placements = best;
                moves += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok((placements, moves))
}
