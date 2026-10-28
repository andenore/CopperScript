//! Transactional placement/global-routing feedback controller (`pcbir.routeflow`).

use indexmap::IndexMap;

use copper_drc::legality::Checker;
use copper_pcb::board::{PhysicalBoard, Placement};
use copper_pcb::transform::transformed_local_point;
use copper_pcb::{Error, Result};
use copper_physical::Point;
use copper_place::PlannerOptions;
use copper_place::planner::{Candidate, generate_placement_candidates};
use copper_units::Decimal;

use crate::progress::{ProgressCallback, emit, int, text};
use crate::routing::{GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus, route_global};

fn decimal_error(error: copper_units::DecimalError) -> Error {
    Error::Value(error.to_string())
}

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackStatus {
    Pass,
    Warning,
}

impl FeedbackStatus {
    pub fn value(self) -> &'static str {
        match self {
            FeedbackStatus::Pass => "pass",
            FeedbackStatus::Warning => "warning",
        }
    }
}

/// `PlacementRoutingFeedbackOptions`, validated by [`PlacementRoutingFeedbackOptions::validated`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementRoutingFeedbackOptions {
    pub maximum_iterations: i64,
    pub initial_movement_nm: i64,
    pub minimum_movement_nm: i64,
    pub maximum_trials_per_iteration: i64,
    pub stagnation_limit: i64,
    pub preferred_candidate_id: Option<String>,
}

impl Default for PlacementRoutingFeedbackOptions {
    fn default() -> Self {
        PlacementRoutingFeedbackOptions {
            maximum_iterations: 4,
            initial_movement_nm: crate::nm_from_mm(5),
            minimum_movement_nm: 500_000,
            maximum_trials_per_iteration: 24,
            stagnation_limit: 3,
            preferred_candidate_id: None,
        }
    }
}

impl PlacementRoutingFeedbackOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if self
            .maximum_iterations
            .min(self.initial_movement_nm)
            .min(self.minimum_movement_nm)
            .min(self.stagnation_limit)
            <= 0
        {
            return fail("placement-routing feedback options must be positive");
        }
        if self.maximum_trials_per_iteration < 0 {
            return fail("placement-routing trial budget must be a nonnegative integer");
        }
        if self.minimum_movement_nm > self.initial_movement_nm {
            return fail("minimum feedback movement exceeds initial movement");
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedbackIteration {
    pub iteration: i64,
    pub accepted: bool,
    pub movement_nm: i64,
    pub trials: i64,
    pub unrouted_net_count: i64,
    pub total_overflow: i64,
    pub maximum_overflow: i64,
    pub routing_fingerprint: String,
}

/// `PlacementRoutingResult` (`package_patterns` is not rewritten: the
/// placement statistics carry no package patterns yet).
#[derive(Clone, Debug)]
pub struct PlacementRoutingResult {
    pub status: FeedbackStatus,
    pub board: PhysicalBoard,
    pub global_route: GlobalRoutingResult,
    pub placement_candidate: String,
    pub iterations: Vec<FeedbackIteration>,
    pub accepted_moves: i64,
    pub full_route_certified: bool,
}

/// Reborrow an optional observer for one call.
fn observer<'s>(callback: &'s mut Option<ProgressCallback<'_>>) -> Option<ProgressCallback<'s>> {
    match callback {
        Some(callback) => Some(&mut **callback),
        None => None,
    }
}

/// `optimize_placement_for_routing`: place, fully global-route, and transactionally relieve congestion.
pub fn optimize_placement_for_routing(
    board: &PhysicalBoard,
    placement_options: &PlannerOptions,
    router_options: &GlobalRouterOptions,
    feedback_options: &PlacementRoutingFeedbackOptions,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<PlacementRoutingResult> {
    let candidates = generate_placement_candidates(board, placement_options)?;
    let mut evaluated = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        evaluated.push(evaluate_candidate(board, candidate, router_options, observer(&mut on_progress))?);
    }
    let (candidate, mut accepted_board, mut accepted_route) = match &feedback_options.preferred_candidate_id {
        Some(preferred) => {
            let Some(position) = evaluated.iter().position(|item| item.0.candidate_id == *preferred) else {
                let available: Vec<&str> = evaluated.iter().map(|item| item.0.candidate_id.as_str()).collect();
                return Err(Error::Value(format!(
                    "placement candidate {} is unavailable; legal candidates: {}",
                    copper_json::python_str_repr(preferred),
                    available.join(", ")
                )));
            };
            evaluated.swap_remove(position)
        }
        None => {
            let position = (0..evaluated.len())
                .min_by_key(|index| {
                    let item = &evaluated[*index];
                    (route_score(&item.2), item.0.metrics.quality_vector(), item.0.seed)
                })
                .expect("at least one placement candidate");
            evaluated.swap_remove(position)
        }
    };
    let mut records = Vec::new();
    let mut accepted_moves = 0;
    let mut movement = feedback_options.initial_movement_nm;
    let mut stagnation = 0;
    for iteration in 1..=feedback_options.maximum_iterations {
        if feedback_options.maximum_trials_per_iteration == 0 {
            break;
        }
        if accepted_route.status == GlobalRoutingStatus::Success {
            break;
        }
        let current_score = route_score(&accepted_route);
        let mut best_trial: Option<([i64; 7], PhysicalBoard, GlobalRoutingResult)> = None;
        let mut trials = 0;
        for trial_board in movement_trials(&accepted_board, &accepted_route, placement_options, movement, &[])? {
            emit(
                &mut on_progress,
                "placement_global_trial",
                "started",
                vec![("iteration", int(iteration)), ("trial", int(trials + 1))],
            );
            let trial_route = route_global(&trial_board, router_options, observer(&mut on_progress), None, None)?;
            emit(
                &mut on_progress,
                "placement_global_trial",
                "finished",
                vec![
                    ("iteration", int(iteration)),
                    ("trial", int(trials + 1)),
                    ("global_status", text(trial_route.status.value())),
                ],
            );
            trials += 1;
            let score = route_score(&trial_route);
            if score < current_score
                && best_trial.as_ref().is_none_or(|best| {
                    (score, &trial_route.routing_fingerprint) < (best.0, &best.2.routing_fingerprint)
                })
            {
                best_trial = Some((score, trial_board, trial_route));
            }
            if trials >= feedback_options.maximum_trials_per_iteration {
                break;
            }
        }
        let accepted = match best_trial {
            Some((_, board, route)) => {
                accepted_board = board;
                accepted_route = route;
                accepted_moves += 1;
                stagnation = 0;
                true
            }
            None => {
                stagnation += 1;
                movement = feedback_options.minimum_movement_nm.max(movement.div_euclid(2));
                false
            }
        };
        records.push(FeedbackIteration {
            iteration,
            accepted,
            movement_nm: movement,
            trials,
            unrouted_net_count: accepted_route.metrics.unrouted_net_count,
            total_overflow: accepted_route.metrics.total_overflow,
            maximum_overflow: accepted_route.metrics.maximum_overflow,
            routing_fingerprint: accepted_route.routing_fingerprint.clone(),
        });
        if stagnation >= feedback_options.stagnation_limit {
            break;
        }
    }
    emit(
        &mut on_progress,
        "placement_global_certification",
        "started",
        vec![("candidate", text(&candidate.candidate_id))],
    );
    let certified = route_global(&accepted_board, router_options, observer(&mut on_progress), None, None)?;
    emit(
        &mut on_progress,
        "placement_global_certification",
        "finished",
        vec![("candidate", text(&candidate.candidate_id)), ("global_status", text(certified.status.value()))],
    );
    let full_route_certified = certified.status == GlobalRoutingStatus::Success;
    Ok(PlacementRoutingResult {
        status: if full_route_certified { FeedbackStatus::Pass } else { FeedbackStatus::Warning },
        board: accepted_board,
        global_route: certified,
        placement_candidate: candidate.candidate_id,
        iterations: records,
        accepted_moves,
        full_route_certified,
    })
}

/// `_evaluate_candidate`.
fn evaluate_candidate(
    board: &PhysicalBoard,
    candidate: Candidate,
    router_options: &GlobalRouterOptions,
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<(Candidate, PhysicalBoard, GlobalRoutingResult)> {
    let mut placed = board.clone();
    placed.placements = candidate.placements.clone();
    emit(
        &mut on_progress,
        "placement_candidate_global",
        "started",
        vec![("candidate", text(&candidate.candidate_id)), ("seed", int(candidate.seed))],
    );
    let routed = route_global(&placed, router_options, observer(&mut on_progress), None, None)?;
    emit(
        &mut on_progress,
        "placement_candidate_global",
        "finished",
        vec![
            ("candidate", text(&candidate.candidate_id)),
            ("seed", int(candidate.seed)),
            ("global_status", text(routed.status.value())),
        ],
    );
    Ok((candidate, placed, routed))
}

/// `_route_score`.
pub fn route_score(route: &GlobalRoutingResult) -> [i64; 7] {
    route.metrics.quality_vector()
}

/// `move_placement_unit` (`pcbir.clusters`): expand a member's proposed pose;
/// callers still apply all acceptance gates.
pub fn move_placement_unit(
    board: &PhysicalBoard,
    placements: &IndexMap<String, Placement>,
    reference: &str,
    proposed: Placement,
) -> Result<IndexMap<String, Placement>> {
    let mut result = placements.clone();
    let Some(cluster) =
        board.rigid_clusters.iter().find(|item| item.members.iter().any(|member| member.reference == reference))
    else {
        result.insert(reference.to_owned(), proposed);
        return Ok(result);
    };
    let local = cluster.members.iter().find(|item| item.reference == reference).expect("cluster member");
    let local_anchor =
        cluster.members.iter().find(|item| item.reference == cluster.anchor_reference).expect("anchor member");
    let full = Decimal::from_i64(360);
    let difference = proposed.rotation_degrees.sub(&local.rotation_degrees).map_err(decimal_error)?;
    let rotation =
        copper_pcb::decimal_rem(&copper_pcb::decimal_rem(&difference, 360)?.add(&full).map_err(decimal_error)?, 360)?;
    let mut frame = proposed.clone();
    frame.rotation_degrees = rotation.clone();
    let Some(anchor) = placements.get(&cluster.anchor_reference) else {
        return Err(Error::Value(copper_json::python_str_repr(&cluster.anchor_reference)));
    };
    let mut anchor = anchor.clone();
    anchor.side = proposed.side;
    anchor.rotation_degrees = rotation;
    anchor.position = transformed_local_point(
        &frame,
        Point {
            x_nm: local_anchor.position.x_nm - local.position.x_nm,
            y_nm: local_anchor.position.y_nm - local.position.y_nm,
        },
    );
    for (key, pose) in copper_pcb::hard_macros::cluster_placements(cluster, &anchor, placements)? {
        result.insert(key, pose);
    }
    Ok(result)
}

/// `_movement_trials`.
pub fn movement_trials(
    board: &PhysicalBoard,
    route: &GlobalRoutingResult,
    placement_options: &PlannerOptions,
    movement_nm: i64,
    extra_nets: &[String],
) -> Result<Vec<PhysicalBoard>> {
    if !board.tracks.is_empty()
        || !board.vias.is_empty()
        || !board.polygons.is_empty()
        || !board.materialized_macros.is_empty()
    {
        // Never displace terminals beneath accepted copper (the IR keeps no
        // zone fills, which only routed boards carry).
        return Ok(Vec::new());
    }
    let mut pressured: Vec<&str> =
        route.hotspots.iter().flat_map(|hotspot| hotspot.contributors.iter().map(String::as_str)).collect();
    pressured.extend(route.routes.iter().filter(|item| !item.connected).map(|item| item.net.as_str()));
    pressured.extend(extra_nets.iter().map(String::as_str));
    let mut references: Vec<&str> = board
        .nets
        .iter()
        .filter(|net| pressured.contains(&net.name.as_str()))
        .flat_map(|net| net.pads.iter().map(|pad| pad.component.as_str()))
        .collect();
    references.sort_unstable();
    references.dedup();
    let mut fixed: Vec<&str> = placement_options.fixed_references.iter().map(String::as_str).collect();
    fixed.extend(board.placement_rules.iter().filter(|r| r.fixed_position.is_some()).map(|r| r.reference.as_str()));
    let placements: IndexMap<String, Placement> =
        board.placements.iter().map(|item| (item.reference.clone(), item.clone())).collect();
    let checker = Checker::new(board, placement_options.policy());
    let mut trials = Vec::new();
    for reference in references {
        if fixed.contains(&reference) {
            continue;
        }
        let Some(current) = placements.get(reference) else {
            return Err(Error::Value(copper_json::python_str_repr(reference)));
        };
        for (dx, dy) in directions_away_from_hotspots(current, route, movement_nm) {
            let mut moved = current.clone();
            moved.position = Point { x_nm: current.position.x_nm + dx, y_nm: current.position.y_nm + dy };
            let candidate = match move_placement_unit(board, &placements, reference, moved) {
                Ok(candidate) => candidate,
                Err(Error::Value(_)) => continue,
                Err(other) => return Err(other),
            };
            if !checker.placement_solution_is_legal(&candidate)? {
                continue;
            }
            let mut trial = board.clone();
            trial.placements = board.placements.iter().map(|item| candidate[&item.reference].clone()).collect();
            trials.push(trial);
        }
    }
    Ok(trials)
}

/// `_directions_away_from_hotspots`.
fn directions_away_from_hotspots(
    placement: &Placement,
    route: &GlobalRoutingResult,
    movement_nm: i64,
) -> Vec<(i64, i64)> {
    let mut ranked: Vec<(i64, i64, i64)> = Vec::new();
    for hotspot in &route.hotspots {
        let center_x = (hotspot.start.x_nm + hotspot.end.x_nm).div_euclid(2);
        let center_y = (hotspot.start.y_nm + hotspot.end.y_nm).div_euclid(2);
        let dx = if placement.position.x_nm >= center_x { movement_nm } else { -movement_nm };
        let dy = if placement.position.y_nm >= center_y { movement_nm } else { -movement_nm };
        ranked.extend([(-hotspot.overflow, dx, 0), (-hotspot.overflow, 0, dy), (-hotspot.overflow, dx, dy)]);
    }
    ranked.extend([(0, -movement_nm, 0), (0, 0, -movement_nm), (0, 0, movement_nm), (0, movement_nm, 0)]);
    ranked.sort_unstable();
    ranked.dedup();
    ranked.into_iter().filter(|(_, dx, dy)| *dx != 0 || *dy != 0).map(|(_, dx, dy)| (dx, dy)).collect()
}
