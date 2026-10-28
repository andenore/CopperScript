//! Detailed improvement passes (`_detailed_refine`, `_repair_relative_constraints`,
//! `_inline_pair_candidates`, `_spread_escape_components`). Each keeps the
//! complete legality gate; a pass that worsens penalty or overflow is undone.

use std::collections::BTreeSet;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::Result;
use copper_pcb::board::{PadReference, Placement, RelativePlacementKind, RelativePlacementRule, RouteKind};
use copper_physical::Point;
use copper_physical::pyfloat::{hypot, round_int};

use crate::escape::EscapeSpacingModel;
use crate::legalize::{adjacency, bounded_local_candidates, local_obstacles, relative_candidates};
use crate::metrics::{coarse_route, fast_score, hpwl, meaningfully_better, placement_metrics};
use crate::options::PlannerOptions;
use crate::scene::{Poses, Scene, moved, posed, without};
use crate::seed::decimal_int;

/// `_nearby_positions`: grid neighbours by L1 ring, then y, then x.
pub fn nearby_positions(center: Point, options: &PlannerOptions) -> Vec<Point> {
    let radius = options.refinement_radius_steps;
    let mut offsets: Vec<(i64, i64)> = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx != 0 || dy != 0 {
                offsets.push((dx, dy));
            }
        }
    }
    offsets.sort_by_key(|(dx, dy)| (dx.abs() + dy.abs(), *dy, *dx));
    offsets
        .into_iter()
        .map(|(dx, dy)| Point::new(center.x_nm + dx * options.grid_step_nm, center.y_nm + dy * options.grid_step_nm))
        .collect()
}

/// `_inline_pair_candidates`: long-range corridor poses for a part joining two differential pairs.
fn inline_pair_candidates(
    scene: &Scene,
    placements: &Poses,
    reference: &str,
    options: &PlannerOptions,
) -> Result<Vec<Placement>> {
    let board = scene.board;
    let nets: IndexMap<&str, &copper_pcb::board::PhysicalNet> =
        board.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    let rules: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let mut groups: Vec<(PadReference, PadReference)> = Vec::new();
    for rule in &board.net_routing_rules {
        let Some(partner) = &rule.differential_partner else { continue };
        let paired = rules.get(partner.as_str());
        if rule.kind != RouteKind::Differential
            || rule.net >= *partner
            || paired
                .is_none_or(|p| p.kind != RouteKind::Differential || p.differential_partner.as_ref() != Some(&rule.net))
        {
            continue;
        }
        let (mut external, mut own): (Vec<&PadReference>, Vec<&PadReference>) = (Vec::new(), Vec::new());
        for name in [&rule.net, partner] {
            let Some(net) = nets.get(name.as_str()).filter(|n| n.pads.len() == 2) else { break };
            let own_pads: Vec<&PadReference> = net.pads.iter().filter(|p| p.component == reference).collect();
            let other_pads: Vec<&PadReference> = net.pads.iter().filter(|p| p.component != reference).collect();
            if own_pads.len() != 1 || other_pads.len() != 1 {
                break;
            }
            own.extend(own_pads);
            external.extend(other_pads);
        }
        let components: IndexSet<&str> = external.iter().map(|p| p.component.as_str()).collect();
        let own_unique: IndexSet<&&PadReference> = own.iter().collect();
        if external.len() == 2 && components.len() == 1 && own_unique.len() == 2 {
            groups.push((external[0].clone(), external[1].clone()));
        }
    }
    if groups.len() != 2 || groups[0].0.component == groups[1].0.component {
        return Ok(Vec::new());
    }
    let current = &placements[reference];
    if groups.iter().any(|g| placements[&g.0.component].side != current.side) {
        return Ok(Vec::new());
    }
    let center = |group: &(PadReference, PadReference)| -> Result<Point> {
        let a = scene.pad_position(&placements[&group.0.component], &group.0.pad)?;
        let b = scene.pad_position(&placements[&group.1.component], &group.1.pad)?;
        Ok(Point::new((a.x_nm + b.x_nm).div_euclid(2), (a.y_nm + b.y_nm).div_euclid(2)))
    };
    let (a, b) = (center(&groups[0])?, center(&groups[1])?);
    let (dx, dy) = (b.x_nm - a.x_nm, b.y_nm - a.y_nm);
    let distance = hypot(dx as f64, dy as f64);
    if distance < (4 * options.grid_step_nm) as f64 {
        return Ok(Vec::new());
    }
    let step = options.grid_step_nm;
    let s = step as f64;
    let mut positions: IndexSet<Point> = IndexSet::new();
    for fraction in [0.5, 0.25, 0.75] {
        for lateral in [0i64, -2, 2, -4, 4, -6, 6, -8, 8] {
            let lateral = lateral as f64;
            let x = a.x_nm as f64 + dx as f64 * fraction - dy as f64 / distance * lateral * s;
            let y = a.y_nm as f64 + dy as f64 * fraction + dx as f64 / distance * lateral * s;
            positions.insert(Point::new(round_int(x / s) * step, round_int(y / s) * step));
        }
    }
    let orientations = scene.allowed_orientations(reference);
    Ok(positions.into_iter().flat_map(|p| orientations.iter().map(move |angle| posed(current, p, angle))).collect())
}

fn pin_limited(scene: &Scene, reference: &str) -> bool {
    scene.relative.iter().any(|r| {
        r.kind == RelativePlacementKind::MaxDistance
            && r.targets.iter().all(|t| t.pad.is_some())
            && r.targets.iter().any(|t| t.reference == reference)
    })
}

/// The refinement statistics `_detailed_refine` returns.
pub struct Refined {
    pub placements: Poses,
    pub moves: i64,
    pub swaps: i64,
    pub feedback_passes: i64,
}

/// `_detailed_refine`: one-component moves among local poses, then adjacent swaps.
pub fn detailed_refine(
    scene: &Scene,
    source: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<Refined> {
    let weight = options.power_domain_weight;
    let mut placements = source.clone();
    let fixed: IndexSet<String> = scene.fixed_placements(source, &options.fixed_references).into_keys().collect();
    let adjacency = adjacency(scene);
    let (mut move_count, mut swap_count, mut feedback_passes) = (0, 0, 0);
    for pass_index in 0..options.refinement_passes {
        let snapshot = placements.clone();
        let before = placement_metrics(scene, &snapshot, options, spacing)?;
        let mut changed = false;
        let mut references: Vec<String> = placements.keys().cloned().collect();
        references.sort();
        for reference in &references {
            if fixed.contains(reference) {
                continue;
            }
            scene.progress(&format!("refine:{}:{reference}", pass_index + 1));
            let current = placements[reference].clone();
            let others = without(&placements, reference);
            let obstacles = local_obstacles(scene, reference, &others);
            let mut best = current.clone();
            let mut best_score = fast_score(scene, &placements, spacing, weight)?;
            let mut positions = vec![current.position];
            positions.extend(nearby_positions(current.position, options));
            let local_candidates = if pin_limited(scene, reference) {
                relative_candidates(scene, &scene.relative, reference, &current, &others, options, 250_000)?
            } else {
                Vec::new()
            };
            let mut candidates: IndexSet<Placement> = IndexSet::new();
            candidates.extend(inline_pair_candidates(scene, &placements, reference, options)?);
            candidates.extend(local_candidates.iter().cloned());
            let orientations = scene.allowed_orientations(reference);
            for point in &positions {
                for angle in &orientations {
                    candidates.insert(posed(&current, *point, angle));
                }
            }
            let mut evaluated = 0;
            for candidate in candidates {
                if !scene.legal(&candidate, &obstacles)? {
                    continue;
                }
                let mut trial = others.clone();
                trial.insert(reference.clone(), candidate.clone());
                if scene.penalty(&trial)? > best_score[0] {
                    continue;
                }
                evaluated += 1;
                let score = fast_score(scene, &trial, spacing, weight)?;
                let ranked =
                    (score, candidate.position.y_nm, candidate.position.x_nm, decimal_int(&candidate.rotation_degrees));
                let best_ranked =
                    (best_score, best.position.y_nm, best.position.x_nm, decimal_int(&best.rotation_degrees));
                if ranked < best_ranked {
                    best = candidate;
                    best_score = score;
                }
                if !local_candidates.is_empty() && evaluated >= options.legalization_candidates {
                    break;
                }
            }
            if best != current {
                placements[reference] = best;
                changed = true;
                move_count += 1;
            }
        }
        let mut pairs: BTreeSet<(String, String)> = BTreeSet::new();
        for (left, neighbours) in &adjacency {
            for right in neighbours.keys() {
                if !fixed.contains(left) && !fixed.contains(right) {
                    pairs.insert(if left <= right {
                        (left.clone(), right.clone())
                    } else {
                        (right.clone(), left.clone())
                    });
                }
            }
        }
        scene.progress(&format!("refine_swaps:{}", pass_index + 1));
        let budget = 8.max(placements.len());
        for (left, right) in pairs.into_iter().take(budget) {
            let (a, b) = (placements[&left].clone(), placements[&right].clone());
            let mut trial = placements.clone();
            trial.shift_remove(&left);
            trial.shift_remove(&right);
            let (swapped_a, swapped_b) = (moved(&a, b.position), moved(&b, a.position));
            if !scene.legal(&swapped_a, &trial)? {
                continue;
            }
            trial.insert(left.clone(), swapped_a);
            if !scene.legal(&swapped_b, &trial)? {
                continue;
            }
            trial.insert(right.clone(), swapped_b);
            let new = fast_score(scene, &trial, spacing, weight)?;
            let old = fast_score(scene, &placements, spacing, weight)?;
            if meaningfully_better(&new, &old) {
                placements = trial;
                changed = true;
                swap_count += 1;
            }
        }
        let after = placement_metrics(scene, &placements, options, spacing)?;
        feedback_passes += 1;
        if after.constraint_penalty_nm > before.constraint_penalty_nm
            || after.congestion_overflow > before.congestion_overflow
        {
            placements = snapshot;
            changed = false;
        }
        if !changed {
            break;
        }
    }
    Ok(Refined { placements, moves: move_count, swaps: swap_count, feedback_passes })
}

/// `_repair_relative_constraints`: bounded local moves of violating components.
pub fn repair_relative_constraints(
    scene: &Scene,
    source: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<(Poses, i64)> {
    let weight = options.power_domain_weight;
    let mut placements = source.clone();
    let fixed: IndexSet<String> = scene.fixed_placements(source, &options.fixed_references).into_keys().collect();
    let mut repair_count = 0;
    let mut remaining = options.legalization_candidates * options.exact_repair_limit * 8;
    let rounds = 1.max(options.exact_repair_limit.min(scene.relative.len() as i64 * 2));
    for _ in 0..rounds {
        if scene.penalty(&placements)? == 0 {
            break;
        }
        let violating: IndexSet<String> = scene
            .violations(&placements)?
            .into_iter()
            .flat_map(|v| v.references)
            .filter(|r| !fixed.contains(r))
            .collect();
        let mut violating: Vec<String> = violating.into_iter().collect();
        violating.sort_by_key(|r| (scene.footprint_area(&placements[r]), r.clone()));
        let mut changed = false;
        for reference in &violating {
            let baseline = scene.penalty(&placements)?;
            if baseline == 0 {
                break;
            }
            let mut best: Option<Poses> = None;
            let mut best_rank = (baseline, fast_score(scene, &placements, spacing, weight)?);
            let current = placements[reference].clone();
            let others = without(&placements, reference);
            let mut evaluated = 0;
            let obstacles = local_obstacles(scene, reference, &others);
            let local_rules: Vec<&RelativePlacementRule> = scene
                .relative
                .iter()
                .copied()
                .filter(|r| r.targets.iter().any(|t| t.reference == *reference))
                .collect();
            let related: IndexSet<&str> =
                local_rules.iter().flat_map(|r| r.targets.iter().map(|t| t.reference.as_str())).collect();
            let anchors: Poses = others
                .iter()
                .filter(|(r, _)| related.contains(r.as_str()))
                .map(|(r, p)| (r.clone(), p.clone()))
                .collect();
            let constant = baseline - scene.relative_penalty(&placements, &local_rules)?;
            let candidates =
                relative_candidates(scene, &scene.relative, reference, &current, &others, options, 100_000)?;
            let limit = 1.max(remaining) as usize;
            for candidate in bounded_local_candidates(candidates, limit, Box::new(std::iter::empty())) {
                if remaining <= 0 {
                    if let Some(best) = best {
                        placements = best;
                        repair_count += 1;
                    }
                    return Ok((placements, repair_count));
                }
                remaining -= 1;
                let mut trial_anchors = anchors.clone();
                trial_anchors.insert(reference.clone(), candidate.clone());
                let penalty = constant + scene.relative_penalty(&trial_anchors, &local_rules)?;
                if penalty >= baseline || penalty > best_rank.0 || !scene.legal(&candidate, &obstacles)? {
                    continue;
                }
                let mut trial = others.clone();
                trial.insert(reference.clone(), candidate);
                let rank = (penalty, fast_score(scene, &trial, spacing, weight)?);
                if rank < best_rank {
                    best_rank = rank;
                    best = Some(trial);
                }
                evaluated += 1;
                if evaluated >= options.exact_repair_candidates {
                    break;
                }
            }
            if let Some(best) = best {
                placements = best;
                repair_count += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok((placements, repair_count))
}

/// `_spread_escape_components`: relieve facing channels by translating complete constrained units.
pub fn spread_escape_components(
    scene: &Scene,
    source: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<(Poses, i64)> {
    let weight = options.power_domain_weight;
    let mut placements = source.clone();
    let fixed: IndexSet<String> = scene.fixed_placements(&placements, &options.fixed_references).into_keys().collect();
    let grid = options.grid_step_nm;
    let maximum = options.escape_max_movement_nm;
    let mut moves = 0;
    for _ in 0..options.escape_spacing_passes {
        let mut changed = false;
        let channels = spacing.channels(&placements, false);
        let references: IndexSet<&str> = channels.iter().flat_map(|c| [c.left.as_str(), c.right.as_str()]).collect();
        let mut ordered: Vec<String> = references.iter().map(|r| (*r).to_owned()).collect();
        ordered.sort_by_key(|r| {
            (spacing.units[r].iter().map(|item| scene.footprint_area(&placements[item])).sum::<i64>(), r.clone())
        });
        let mut visited: Vec<*const BTreeSet<String>> = Vec::new();
        for reference in &ordered {
            let cluster = spacing.units[reference].clone();
            if visited.contains(&std::rc::Rc::as_ptr(&cluster)) {
                continue;
            }
            visited.push(std::rc::Rc::as_ptr(&cluster));
            if cluster.iter().any(|r| fixed.contains(r)) {
                continue;
            }
            scene.progress(&format!("escape_unit:{reference}"));
            let relevant: Vec<crate::escape::EscapeChannel> = spacing
                .channels(&placements, false)
                .into_iter()
                .filter(|c| cluster.contains(&c.left) != cluster.contains(&c.right))
                .collect();
            if relevant.is_empty() {
                continue; // Internal proximity deficits cannot be relieved by translation.
            }
            let mut offsets: IndexSet<(i64, i64)> = IndexSet::new();
            for channel in &relevant {
                let (own, other) = if cluster.contains(&channel.left) {
                    (&channel.left, &channel.right)
                } else {
                    (&channel.right, &channel.left)
                };
                let (a, b) = (placements[own].position, placements[other].position);
                let less = if channel.axis == "x" { a.x_nm < b.x_nm } else { a.y_nm < b.y_nm };
                let sign = if less { -1 } else { 1 };
                let amount = (channel.deficit_nm() + grid - 1).div_euclid(grid) * grid;
                let amount = maximum.min(amount).div_euclid(grid) * grid;
                if amount != 0 {
                    offsets.insert(if channel.axis == "x" { (sign * amount, 0) } else { (0, sign * amount) });
                }
            }
            for radius in [grid, 2_000_000, 4_000_000, 8_000_000, maximum] {
                if radius > maximum {
                    continue;
                }
                let radius = radius.div_euclid(grid) * grid;
                if radius == 0 {
                    continue;
                }
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (-1, 1), (1, -1), (1, 1)] {
                    let (x, y) = (i128::from(dx * radius), i128::from(dy * radius));
                    if x * x + y * y <= i128::from(maximum) * i128::from(maximum) {
                        offsets.insert((dx * radius, dy * radius));
                    }
                }
            }
            let baseline = fast_score(scene, &placements, spacing, weight)?;
            let mut best: Option<Poses> = None;
            let mut best_rank = (baseline, hpwl(scene, &placements)?);
            let mut offsets: Vec<(i64, i64)> = offsets.into_iter().collect();
            offsets.sort_by_key(|(x, y)| (x.abs() + y.abs(), *y, *x));
            for (offset_x, offset_y) in offsets {
                let mut trial = placements.clone();
                for item in cluster.iter() {
                    let current = &trial[item];
                    let position = Point::new(current.position.x_nm + offset_x, current.position.y_nm + offset_y);
                    trial[item] = moved(current, position);
                }
                let rank = (fast_score(scene, &trial, spacing, weight)?, hpwl(scene, &trial)?);
                // Only an improving proposal can be selected; it still receives the full check.
                if rank < best_rank && scene.checker.placement_solution_is_legal(&trial)? {
                    best = Some(trial);
                    best_rank = rank;
                }
            }
            if let Some(best) = best
                && coarse_route(scene, &best, options)?.overflow <= coarse_route(scene, &placements, options)?.overflow
            {
                placements = best;
                changed = true;
                moves += 1;
            }
        }
        if !changed {
            break;
        }
    }
    Ok((placements, moves))
}
