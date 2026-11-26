//! The candidate pipeline (`generate_placement_candidates`,
//! `select_placement_candidate`): per seed, analytical placement, rigid
//! clusters, legalization, relative repair, refinement, escape spacing and
//! decoupling access; then the Pareto frontier of legal candidates.

use std::sync::OnceLock;

use copper_drc::macros::{MacroMaterialization, MacroRejection};
use indexmap::IndexSet;

use copper_pcb::board::{PhysicalBoard, Placement};
use copper_pcb::keepouts::placement_map;
use copper_pcb::{Error, Result};

use crate::clusters::{ClusterPlacement, refine_rigid_clusters, try_place_rigid_clusters};
use crate::escape::EscapeSpacingModel;
use crate::legalize::{Legalized, legalize_with_reservations, local_failure_details};
use crate::metrics::{PlacementMetrics, dominates, placement_metrics};
use crate::options::PlannerOptions;
use crate::refine::{detailed_refine, repair_relative_constraints, spread_escape_components};
use crate::scene::{PlacementProgress, Poses, Scene};
use crate::seed::{analytical_place, initial_seed};

/// `PlacementSeedFailure`: a discarded bounded attempt, never a partial placement certificate.
#[derive(Clone, Debug, PartialEq)]
pub struct SeedFailure {
    pub seed: i64,
    pub failed_reference: String,
    pub accepted_count: i64,
    pub reason: String,
}

/// Inspection-only snapshot of a bounded failed seed. Held poses are not a
/// certified placement; the proposal is the failed component's analytical target.
/// A complete constraint-rejected layout has no unaccepted proposal.
#[derive(Clone, Debug)]
pub struct PlacementFailureDiagnostic {
    pub seed: i64,
    pub failed_reference: String,
    pub accepted: Poses,
    pub proposal: Option<Placement>,
    pub reason: String,
    /// Rejected immutable trial copper for inspection only; never a checkpoint.
    pub physical_rejection: Option<Box<MacroRejection>>,
    /// Compact observation of the failed ordinary local pack; no candidate poses.
    pub bounded_packing: Option<copper_json::Value>,
    /// True when this discarded seed accompanies a successful search.
    pub search_succeeded: bool,
}

pub type PlacementFailureObserver<'a> = &'a mut dyn FnMut(&PlacementFailureDiagnostic);

/// `PlacementStatistics` (support and package patterns are not implemented).
#[derive(Clone, Debug, PartialEq)]
pub struct Statistics {
    pub analytical_iterations: i64,
    pub exact_repair_count: i64,
    pub relative_repair_count: i64,
    pub refinement_move_count: i64,
    pub refinement_swap_count: i64,
    pub routing_feedback_passes: i64,
    pub failed_seed_attempts: Vec<SeedFailure>,
}

/// `PlacementCandidate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub seed: i64,
    pub placements: Vec<Placement>,
    pub metrics: PlacementMetrics,
    pub statistics: Statistics,
}

fn error<T>(message: String) -> Result<T> {
    Err(Error::Value(message))
}

fn repr(text: &str) -> String {
    copper_json::python_str_repr(text)
}

/// The decoupling-access pass (`decoupling.improve_decoupling_placement`):
/// `(bypass_board, refined, phase options) -> (poses, moves)`. It runs the
/// decoupler router, which lives in `copper-route` (a dependent of this crate),
/// so `copper_route::install_hooks` registers it (the routing entry points and
/// the binary call it).
pub type DecouplingPass =
    fn(&PhysicalBoard, &Poses, &PlannerOptions, Option<&mut dyn FnMut(&str)>) -> Result<(Poses, i64)>;

static DECOUPLING_PASS: OnceLock<DecouplingPass> = OnceLock::new();

/// Install the decoupling-access pass; the first registration wins.
pub fn register_decoupling_pass(pass: DecouplingPass) {
    let _ = DECOUPLING_PASS.set(pass);
}

/// `improve_decoupling_placement`: the source checks and early exits; a movable
/// decoupler runs the registered pass on the board at the refined poses.
enum DecouplingPlacement {
    Accepted(Poses, i64),
    Rejected(Box<MacroRejection>),
}

fn improve_decoupling_placement(
    scene: &Scene,
    refined: &Poses,
    options: &PlannerOptions,
) -> Result<DecouplingPlacement> {
    let board = scene.board;
    if board.decoupling_links.is_empty() && board.hard_macros.is_empty() {
        return Ok(DecouplingPlacement::Accepted(refined.clone(), 0));
    }
    let mut fixed: IndexSet<String> = scene.fixed_placements(refined, &options.fixed_references).into_keys().collect();
    fixed.extend(board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.clone())));
    for rule in board.via_in_pad_rules.iter().filter(|r| r.rows.is_some()) {
        if !refined.contains_key(&rule.pad.component) {
            return copper_pcb::not_implemented("a via-in-pad array owner without a pose (Python crashes)");
        }
        fixed.insert(rule.pad.component.clone());
    }
    let mut frame = board.clone();
    frame.placements = refined.values().cloned().collect();
    if let MacroMaterialization::Rejected(rejection) = copper_drc::macros::try_materialize_hard_macros(frame)? {
        return Ok(DecouplingPlacement::Rejected(rejection));
    }
    if board.decoupling_links.is_empty()
        || options.decoupling_placement_trials == 0
        || board.decoupling_links.iter().all(|link| fixed.contains(&link.capacitor.component))
    {
        return Ok(DecouplingPlacement::Accepted(refined.clone(), 0));
    }
    let Some(pass) = DECOUPLING_PASS.get() else {
        return copper_pcb::not_implemented(
            "decoupling-access placement without the decoupler router (call copper_route::install_hooks first)",
        );
    };
    let mut bypass_board = board.clone();
    bypass_board.placements = board.placements.iter().map(|p| refined[&p.reference].clone()).collect();
    let mut observe = |stage: &str| scene.progress(stage);
    let (poses, moves) =
        pass(&bypass_board, refined, options, if scene.observed() { Some(&mut observe) } else { None })?;
    Ok(DecouplingPlacement::Accepted(poses, moves))
}

/// `generate_placement_candidates`: a deterministic Pareto frontier of legal candidates.
pub fn generate_placement_candidates(board: &PhysicalBoard, options: &PlannerOptions) -> Result<Vec<Candidate>> {
    generate_placement_candidates_with_progress(board, options, None)
}

/// `generate_placement_candidates(progress=)`: `progress(stage, seed, seed_count)`
/// observes each seed's phases.
pub fn generate_placement_candidates_with_progress<'a>(
    board: &'a PhysicalBoard,
    options: &PlannerOptions,
    progress: Option<PlacementProgress<'a>>,
) -> Result<Vec<Candidate>> {
    generate_placement_candidates_with_diagnostics(board, options, progress, None)
}

/// Observe the deepest bounded failure only when the complete search fails.
/// Discarded seeds remain statistics of successful candidates, not artifacts.
pub fn generate_placement_candidates_with_diagnostics<'a>(
    board: &'a PhysicalBoard,
    options: &PlannerOptions,
    progress: Option<PlacementProgress<'a>>,
    mut on_failure: Option<PlacementFailureObserver<'_>>,
) -> Result<Vec<Candidate>> {
    if !board.tracks.is_empty() || !board.vias.is_empty() {
        return error(
            "placement planning requires an unrouted board; existing copper cannot be moved safely".to_owned(),
        );
    }
    let scene = Scene::new(board, options.policy()).with_progress(progress);
    let placements = placement_map(board);
    let mut unknown: Vec<&String> = options.fixed_references.iter().filter(|r| !placements.contains_key(*r)).collect();
    unknown.sort();
    if let Some(first) = unknown.first() {
        return error(format!("fixed placement references unknown component {}", repr(first)));
    }
    // Diagnose contradictory source/session locks before any soft phase.
    let all_fixed = scene.fixed_placements(&placements, &options.fixed_references);
    let fixed_poses: Poses = all_fixed
        .iter()
        .filter(|(r, _)| {
            options.fixed_references.contains(*r)
                || scene.rule(r).is_some_and(|rule| rule.fixed_rotation_degrees.is_some())
        })
        .map(|(r, p)| (r.clone(), p.clone()))
        .collect();
    let violations = scene.violations(&fixed_poses)?;
    if !violations.is_empty() {
        let messages: Vec<String> = violations.into_iter().map(|v| v.message).collect();
        return error(format!("conflicting fixed placement constraints: {}", messages.join("; ")));
    }
    let mut conflicts: Vec<String> = Vec::new();
    for corridor in scene.checker.reserved_corridors()? {
        let mut references: Vec<&String> = all_fixed.keys().collect();
        references.sort();
        for reference in references {
            if corridor.terminal_references.contains(reference) {
                continue;
            }
            let pose = &all_fixed[reference];
            if copper_drc::legality::keepout_blocks(board, &corridor.keepout, pose, &scene.polygon(pose)) {
                conflicts.push(format!(
                    "reserved corridor {} for differential pair {}/{} overlaps fixed component {}; move {reference} \
                     out of the corridor or remove reserve_corridor from the pair's routing profile",
                    repr(&corridor.keepout.name),
                    corridor.nets.0,
                    corridor.nets.1,
                    repr(reference)
                ));
            }
        }
    }
    if !conflicts.is_empty() {
        return error(conflicts.join("; "));
    }
    // Fully specified poses cannot be repaired by the analytical pass.
    let mut accepted = Poses::new();
    let mut references: Vec<&String> = fixed_poses.keys().collect();
    references.sort();
    for reference in references {
        let pose = &fixed_poses[reference];
        if !scene.legal(pose, &accepted)? {
            let mut blockers = Vec::new();
            if scene.legal(pose, &Poses::new())? {
                for (other, held) in &accepted {
                    let single: Poses = [(other.clone(), held.clone())].into_iter().collect();
                    if !scene.legal(pose, &single)? {
                        blockers.push(other.clone());
                    }
                }
            }
            let suffix =
                if blockers.is_empty() { String::new() } else { format!("; fixed blockers: {}", blockers.join(", ")) };
            return error(format!(
                "fixed component {} violates board, keepout, region, or clearance constraints{suffix}",
                repr(reference)
            ));
        }
        accepted.insert(reference.clone(), pose.clone());
    }
    let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes)?;
    let seeds = options.seed_count.unwrap_or(2.max(options.candidate_count));
    let mut attempts: Vec<Candidate> = Vec::new();
    let mut closest_rejected: Option<(i64, i64, Poses)> = None;
    let mut failures: Vec<SeedFailure> = Vec::new();
    let mut diagnostic: Option<PlacementFailureDiagnostic> = None;
    let mut physical_diagnostic: Option<PlacementFailureDiagnostic> = None;
    let mut remember = |seed, failure: &crate::legalize::LegalizationFailure, targets: &Poses| {
        if on_failure.is_some()
            && diagnostic.as_ref().is_none_or(|previous| failure.accepted.len() > previous.accepted.len())
        {
            diagnostic = Some(PlacementFailureDiagnostic {
                seed,
                failed_reference: failure.failed_reference.clone(),
                accepted: failure.accepted.clone(),
                proposal: targets.get(&failure.failed_reference).cloned(),
                reason: failure.message.clone(),
                physical_rejection: None,
                bounded_packing: failure.bounded_packing.clone(),
                search_succeeded: false,
            });
        }
    };
    for seed in 0..seeds {
        scene.progress_at("analytical", seed, seeds);
        let continuous = initial_seed(&scene, &placements, options, seed)?;
        let continuous = analytical_place(&scene, &continuous, options, &spacing)?;
        scene.progress("legalization");
        let (continuous, seeded_original, phase, reservations) =
            match try_place_rigid_clusters(&scene, &continuous, &placements, options, &spacing)? {
                ClusterPlacement::Placed(targets, original, phase, reservations) => {
                    (targets, original, phase, reservations)
                }
                ClusterPlacement::Failed(failure) => {
                    remember(seed, &failure, &continuous);
                    failures.push(SeedFailure {
                        seed,
                        failed_reference: failure.failed_reference,
                        accepted_count: failure.accepted.len() as i64,
                        reason: failure.message,
                    });
                    scene.progress("legalization_failed");
                    continue;
                }
            };
        let (legalized, exact_repairs) = match legalize_with_reservations(
            &scene,
            &continuous,
            &seeded_original,
            &reservations,
            &phase,
            &spacing,
            seed,
        )? {
            Legalized::Placed(poses, repairs) => (poses, repairs),
            Legalized::Failed(failure) => {
                remember(seed, &failure, &continuous);
                failures.push(SeedFailure {
                    seed,
                    failed_reference: failure.failed_reference,
                    accepted_count: failure.accepted.len() as i64,
                    reason: failure.message,
                });
                scene.progress("legalization_failed");
                continue;
            }
        };
        let (legalized, relative_before) = repair_relative_constraints(&scene, &legalized, &phase, &spacing)?;
        scene.progress("refinement");
        let refined = detailed_refine(&scene, &legalized, &phase, &spacing)?;
        let (mut moves, swaps, feedback_passes) = (refined.moves, refined.swaps, refined.feedback_passes);
        let (refined, relative_after) = repair_relative_constraints(&scene, &refined.placements, &phase, &spacing)?;
        // A bounded relative repair can leave an infeasible seed; keep it as diagnostic evidence.
        let penalty = scene.penalty(&refined)?;
        if penalty != 0 || !scene.checker.placement_solution_is_legal(&refined)? {
            if closest_rejected.as_ref().is_none_or(|(best, _, _)| penalty < *best) {
                closest_rejected = Some((penalty, seed, refined));
            }
            scene.progress("constraints_rejected");
            continue;
        }
        scene.progress("cluster_refinement");
        let (refined, cluster_moves) = refine_rigid_clusters(&scene, &refined, options, &spacing)?;
        moves += cluster_moves;
        scene.progress("escape_spacing");
        let (refined, spacing_moves) = spread_escape_components(&scene, &refined, options, &spacing)?;
        moves += spacing_moves;
        scene.progress("decoupling_access");
        let (refined, bypass_moves) = match improve_decoupling_placement(&scene, &refined, &phase)? {
            DecouplingPlacement::Accepted(poses, moves) => (poses, moves),
            DecouplingPlacement::Rejected(rejection) => {
                let reason = rejection.message();
                let reference = rejection.failed_reference().unwrap_or_default().to_owned();
                failures.push(SeedFailure {
                    seed,
                    failed_reference: reference.clone(),
                    accepted_count: refined.len() as i64,
                    reason: reason.clone(),
                });
                if physical_diagnostic.is_none() {
                    physical_diagnostic = Some(PlacementFailureDiagnostic {
                        seed,
                        failed_reference: reference,
                        accepted: refined,
                        proposal: None,
                        reason,
                        physical_rejection: Some(rejection),
                        bounded_packing: None,
                        search_succeeded: false,
                    });
                }
                scene.progress("macro_physical_rejected");
                continue;
            }
        };
        moves += bypass_moves;
        if options.support_pattern_trials != 0 {
            scene.progress("support_patterns");
            return copper_pcb::not_implemented("support-pattern placement trials");
        }
        if options.package_support_trials != 0 {
            return copper_pcb::not_implemented("package-support placement trials");
        }
        let metrics = placement_metrics(&scene, &refined, options, &spacing)?;
        if metrics.constraint_penalty_nm != 0 || !scene.checker.placement_solution_is_legal(&refined)? {
            if closest_rejected.as_ref().is_none_or(|(best, _, _)| metrics.constraint_penalty_nm < *best) {
                closest_rejected = Some((metrics.constraint_penalty_nm, seed, refined));
            }
            continue;
        }
        attempts.push(Candidate {
            candidate_id: format!("candidate-{seed:02}"),
            seed,
            placements: board.placements.iter().map(|p| refined[&p.reference].clone()).collect(),
            metrics,
            statistics: Statistics {
                analytical_iterations: options.analytical_iterations,
                exact_repair_count: exact_repairs,
                relative_repair_count: relative_before + relative_after,
                refinement_move_count: moves,
                refinement_swap_count: swaps,
                routing_feedback_passes: feedback_passes,
                failed_seed_attempts: Vec::new(),
            },
        });
    }
    if attempts.is_empty() {
        let mut message = "no legal placement candidate satisfies all represented physical constraints".to_owned();
        if let Some((_, seed, poses)) = &closest_rejected {
            let details = local_failure_details(&scene, poses, options)?;
            if !details.is_empty() {
                message.push_str(&format!("; {details}"));
            }
            // Keep the same lowest-penalty complete rejection used by the error.
            // It is deeper than a bounded partial failure, but is never certified.
            if on_failure.is_some() {
                diagnostic = Some(rejected_layout_diagnostic(&scene, *seed, poses, details)?);
            }
        }
        if !failures.is_empty() {
            let best = failures.iter().min_by_key(|f| (-f.accepted_count, f.seed)).expect("failures");
            let attempts: Vec<String> = failures
                .iter()
                .map(|f| {
                    format!("seed {}: {} after {} accepted placements", f.seed, f.failed_reference, f.accepted_count)
                })
                .collect();
            message.push_str(&format!("; {}; bounded seed attempts: {}", best.reason, attempts.join("; ")));
        }
        if let Some(physical) = physical_diagnostic
            && diagnostic.as_ref().is_none_or(|old| {
                physical.accepted.len() > old.accepted.len()
                    || (physical.accepted.len() == old.accepted.len() && physical.seed < old.seed)
            })
        {
            diagnostic = Some(physical);
        }
        if let (Some(observer), Some(diagnostic)) = (&mut on_failure, &diagnostic) {
            observer(diagnostic);
        }
        return error(message);
    }
    if !failures.is_empty() {
        for candidate in &mut attempts {
            candidate.statistics.failed_seed_attempts = failures.clone();
        }
    }
    let mut frontier: Vec<Candidate> = attempts
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            !attempts
                .iter()
                .enumerate()
                .any(|(other, item)| other != *index && dominates(&item.metrics, &candidate.metrics))
        })
        .map(|(_, candidate)| candidate.clone())
        .collect();
    frontier.sort_by_key(|c| (c.metrics.quality_vector(), c.seed));
    scene.progress_at("finished", seeds, seeds);
    frontier.truncate(options.candidate_count as usize);
    Ok(frontier)
}

fn rejected_layout_diagnostic(
    scene: &Scene,
    seed: i64,
    poses: &Poses,
    details: String,
) -> Result<PlacementFailureDiagnostic> {
    let violations = scene.violations(poses)?;
    let mut reference = violations.iter().flat_map(|v| v.references.iter()).find(|r| poses.contains_key(*r)).cloned();
    if reference.is_none() {
        let keepouts = copper_pcb::keepouts::resolved_cluster_keepouts(scene.board, poses);
        let mut references: Vec<&String> = poses.keys().collect();
        references.sort();
        for candidate in references {
            if !scene.checker.legal(&poses[candidate], &crate::scene::without(poses, candidate), Some(&keepouts))? {
                reference = Some(candidate.clone());
                break;
            }
        }
    }
    Ok(PlacementFailureDiagnostic {
        seed,
        // Global inventory or rigid/fixed invariants need not identify one
        // illegal pose. Use a stable location while keeping the rejection explicit.
        failed_reference: reference.or_else(|| poses.keys().min().cloned()).unwrap_or_default(),
        accepted: poses.clone(),
        proposal: None,
        reason: format!("complete layout rejected by final placement constraints; {details}"),
        physical_rejection: None,
        bounded_packing: None,
        search_succeeded: false,
    })
}

/// `select_placement_candidate`: the best quality vector, then the lowest seed.
pub fn select_placement_candidate(candidates: &[Candidate]) -> Result<&Candidate> {
    candidates
        .iter()
        .min_by_key(|c| (c.metrics.quality_vector(), c.seed))
        .ok_or_else(|| Error::Value("cannot select from an empty candidate set".to_owned()))
}
