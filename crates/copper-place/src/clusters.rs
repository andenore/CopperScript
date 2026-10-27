//! Bounded rigid-cluster legalization and whole-cluster refinement
//! (`pcbir.cluster_placement`). Ordinary components never move a member alone.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{Placement, RigidPlacementCluster};
use copper_pcb::hard_macros::cluster_placements;
use copper_pcb::keepouts::resolved_cluster_keepouts;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::{Error, Result};
use copper_physical::Point;
use copper_units::Decimal;

use crate::escape::{EscapeSpacingModel, placement_units};
use crate::legalize::{
    adjacency, best_legal_choice, candidate_positions, pack_local_components, relative_cluster_order,
};
use crate::metrics::{fast_score, hpwl, placement_metrics};
use crate::options::PlannerOptions;
use crate::refine::nearby_positions;
use crate::scene::{Poses, Scene, posed, with};

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
    let board = scene.board;
    if board.rigid_clusters.is_empty() {
        return Ok((targets.clone(), original.clone(), options.clone()));
    }
    let (mut targets, mut original) = (targets.clone(), original.clone());
    let fixed = scene.fixed_placements(&original, &options.fixed_references);
    let members: IndexSet<String> =
        board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.clone())).collect();
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
    clusters.sort_by_key(|c| (!c.members.iter().any(|m| fixed.contains_key(&m.reference)), c.name.clone()));
    let mut reserved = false;
    for cluster in clusters {
        if !reserved && !cluster.members.iter().any(|m| fixed.contains_key(&m.reference)) {
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
                    return error(format!(
                        "cannot reserve fixed-anchor companion {} before movable rigid clusters",
                        copper_json::python_str_repr(reference)
                    ));
                }
                let choice = choice.expect("a choice");
                placed.insert(reference.clone(), choice.clone());
                targets.insert(reference.clone(), choice);
            }
            reserved = true;
        }
        let local_anchor = cluster.members.iter().find(|m| m.reference == cluster.anchor_reference).expect("anchor");
        let mut target = targets[&cluster.anchor_reference].clone();
        let fixed_members: Vec<&copper_pcb::board::RigidPlacementMember> =
            cluster.members.iter().filter(|m| fixed.contains_key(&m.reference)).collect();
        let poses: Vec<Placement> = if let Some(member) = fixed_members.first() {
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
            positions
                .into_iter()
                .take(options.legalization_candidates as usize)
                .flat_map(|point| cluster.allowed_rotations.iter().map(move |angle| (point, angle.clone())))
                .map(|(point, angle)| posed(&target, point, &angle))
                .collect()
        };
        let mut best: Option<((i64, i64, i64, i64), Decimal, IndexMap<String, Placement>)> = None;
        for pose in &poses {
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
            for item in trial.values() {
                let others = crate::scene::without(&trial, &item.reference);
                if !scene.checker.legal(item, &others, Some(&keepouts))? {
                    legal = false;
                    break;
                }
            }
            if !legal {
                continue;
            }
            let mut merged = targets.clone();
            for (reference, member) in &trial {
                merged.insert(reference.clone(), member.clone());
            }
            let rank = (
                hpwl(scene, &merged)?,
                (pose.position.x_nm - target.position.x_nm).abs() + (pose.position.y_nm - target.position.y_nm).abs(),
                pose.position.y_nm,
                pose.position.x_nm,
            );
            let better = match &best {
                None => true,
                Some((key, angle, _)) => (&rank, &pose.rotation_degrees) < (key, angle),
            };
            if better {
                best = Some((rank, pose.rotation_degrees.clone(), placed_members));
            }
        }
        let Some((_, _, chosen)) = best else {
            return error(format!(
                "cannot legalize rigid cluster {} within its candidate budget",
                copper_json::python_str_repr(&cluster.name)
            ));
        };
        for (reference, pose) in chosen {
            placed.insert(reference.clone(), pose.clone());
            original.insert(reference.clone(), pose.clone());
            targets.insert(reference, pose);
        }
    }
    let mut phase = options.clone();
    phase.fixed_references.extend(members);
    Ok((targets, original, phase))
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
