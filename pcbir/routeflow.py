"""Transactional placement/global-routing feedback controller."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum

from .physical import PhysicalBoard, Placement, Point, nm_from_mm
from .clusters import move_placement_unit
from .placement import (
    PlacementCandidate,
    PlacementPlannerOptions,
    generate_placement_candidates,
    placement_solution_is_legal,
)
from .routing import (
    GlobalRouterOptions,
    GlobalRoutingResult,
    GlobalRoutingStatus,
    route_global,
)


class FeedbackStatus(str, Enum):
    PASS = "pass"
    WARNING = "warning"


@dataclass(frozen=True, slots=True)
class PlacementRoutingFeedbackOptions:
    maximum_iterations: int = 4
    initial_movement_nm: int = nm_from_mm("5")
    minimum_movement_nm: int = nm_from_mm("0.5")
    maximum_trials_per_iteration: int = 24
    stagnation_limit: int = 3
    preferred_candidate_id: str | None = None

    def __post_init__(self) -> None:
        if min(
            self.maximum_iterations,
            self.initial_movement_nm,
            self.minimum_movement_nm,
            self.maximum_trials_per_iteration,
            self.stagnation_limit,
        ) <= 0:
            raise ValueError("placement-routing feedback options must be positive")
        if self.minimum_movement_nm > self.initial_movement_nm:
            raise ValueError("minimum feedback movement exceeds initial movement")


@dataclass(frozen=True, slots=True)
class FeedbackIteration:
    iteration: int
    accepted: bool
    movement_nm: int
    trials: int
    unrouted_net_count: int
    total_overflow: int
    maximum_overflow: int
    routing_fingerprint: str


@dataclass(frozen=True, slots=True)
class PlacementRoutingResult:
    status: FeedbackStatus
    board: PhysicalBoard
    global_route: GlobalRoutingResult
    placement_candidate: str
    iterations: tuple[FeedbackIteration, ...]
    accepted_moves: int
    full_route_certified: bool


def optimize_placement_for_routing(
    board: PhysicalBoard,
    placement_options: PlacementPlannerOptions | None = None,
    router_options: GlobalRouterOptions | None = None,
    feedback_options: PlacementRoutingFeedbackOptions | None = None,
) -> PlacementRoutingResult:
    """Place, fully global-route, and transactionally relieve congestion."""

    placement_options = placement_options or PlacementPlannerOptions()
    router_options = router_options or GlobalRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=router_options.tile_size_nm
    )
    candidates = generate_placement_candidates(board, placement_options)
    evaluated = [
        _evaluate_candidate(board, candidate, router_options)
        for candidate in candidates
    ]
    if feedback_options.preferred_candidate_id is not None:
        selected = [item for item in evaluated
                    if item[0].candidate_id == feedback_options.preferred_candidate_id]
        if not selected:
            available = ", ".join(item[0].candidate_id for item in evaluated)
            raise ValueError(
                f"placement candidate {feedback_options.preferred_candidate_id!r} "
                f"is unavailable; legal candidates: {available}"
            )
        candidate, accepted_board, accepted_route = selected[0]
    else:
        candidate, accepted_board, accepted_route = min(
            evaluated,
            key=lambda item: (
                *_route_score(item[2]),
                *item[0].metrics.quality_vector,
                item[0].seed,
            ),
        )
    records: list[FeedbackIteration] = []
    accepted_moves = 0
    movement = feedback_options.initial_movement_nm
    stagnation = 0
    for iteration in range(1, feedback_options.maximum_iterations + 1):
        if accepted_route.status is GlobalRoutingStatus.SUCCESS:
            break
        current_score = _route_score(accepted_route)
        best_trial: tuple[tuple[int, ...], PhysicalBoard, GlobalRoutingResult] | None = None
        trials = 0
        for trial_board in _movement_trials(
            accepted_board,
            accepted_route,
            placement_options,
            movement,
        ):
            trial_route = route_global(trial_board, router_options)
            trials += 1
            score = _route_score(trial_route)
            if score < current_score and (
                best_trial is None
                or (score, trial_route.routing_fingerprint)
                < (best_trial[0], best_trial[2].routing_fingerprint)
            ):
                best_trial = score, trial_board, trial_route
            if trials >= feedback_options.maximum_trials_per_iteration:
                break
        if best_trial is not None:
            _, accepted_board, accepted_route = best_trial
            accepted_moves += 1
            stagnation = 0
            accepted = True
        else:
            stagnation += 1
            movement = max(feedback_options.minimum_movement_nm, movement // 2)
            accepted = False
        records.append(
            FeedbackIteration(
                iteration,
                accepted,
                movement,
                trials,
                accepted_route.metrics.unrouted_net_count,
                accepted_route.metrics.total_overflow,
                accepted_route.metrics.maximum_overflow,
                accepted_route.routing_fingerprint,
            )
        )
        if stagnation >= feedback_options.stagnation_limit:
            break

    certified = route_global(accepted_board, router_options)
    full_route_certified = certified.status is GlobalRoutingStatus.SUCCESS
    return PlacementRoutingResult(
        FeedbackStatus.PASS if full_route_certified else FeedbackStatus.WARNING,
        accepted_board,
        certified,
        candidate.candidate_id,
        tuple(records),
        accepted_moves,
        full_route_certified,
    )


def _evaluate_candidate(
    board: PhysicalBoard,
    candidate: PlacementCandidate,
    router_options: GlobalRouterOptions,
) -> tuple[PlacementCandidate, PhysicalBoard, GlobalRoutingResult]:
    placed = replace(board, placements=candidate.placements)
    return candidate, placed, route_global(placed, router_options)


def _route_score(route: GlobalRoutingResult) -> tuple[int, ...]:
    metrics = route.metrics
    return (
        metrics.unrouted_net_count,
        metrics.total_overflow,
        metrics.maximum_overflow,
        metrics.overfull_resource_count,
        metrics.region_only_access_count,
        metrics.proposed_via_count,
        metrics.total_length_nm,
    )


def _movement_trials(
    board: PhysicalBoard,
    route: GlobalRoutingResult,
    placement_options: PlacementPlannerOptions,
    movement_nm: int,
) -> tuple[PhysicalBoard, ...]:
    routes_by_net = {item.net: item for item in route.routes}
    pressured_nets = {
        net
        for hotspot in route.hotspots
        for net in hotspot.contributors
    } | {net for net, item in routes_by_net.items() if not item.connected}
    references = sorted(
        {
            pad.component
            for net in board.nets
            if net.name in pressured_nets
            for pad in net.pads
        }
    )
    fixed = set(placement_options.fixed_references) | {
        rule.reference
        for rule in board.placement_rules
        if rule.fixed_position is not None
    }
    placements = {item.reference: item for item in board.placements}
    trials: list[PhysicalBoard] = []
    for reference in references:
        if reference in fixed:
            continue
        current = placements[reference]
        directions = _directions_away_from_hotspots(current, route, movement_nm)
        for dx, dy in directions:
            moved = replace(
                current,
                position=Point(
                    current.position.x_nm + dx,
                    current.position.y_nm + dy,
                ),
            )
            try:
                candidate = move_placement_unit(board, placements, reference, moved)
            except ValueError:
                continue
            if not placement_solution_is_legal(board, candidate, placement_options):
                continue
            ordered = tuple(candidate[item.reference] for item in board.placements)
            trials.append(replace(board, placements=ordered))
    return tuple(trials)


def _directions_away_from_hotspots(
    placement: Placement,
    route: GlobalRoutingResult,
    movement_nm: int,
) -> tuple[tuple[int, int], ...]:
    ranked: list[tuple[int, int, int]] = []
    for hotspot in route.hotspots:
        center_x = (hotspot.start.x_nm + hotspot.end.x_nm) // 2
        center_y = (hotspot.start.y_nm + hotspot.end.y_nm) // 2
        dx = movement_nm if placement.position.x_nm >= center_x else -movement_nm
        dy = movement_nm if placement.position.y_nm >= center_y else -movement_nm
        ranked.extend(
            (
                (-hotspot.overflow, dx, 0),
                (-hotspot.overflow, 0, dy),
                (-hotspot.overflow, dx, dy),
            )
        )
    ranked.extend(
        (
            (0, -movement_nm, 0),
            (0, 0, -movement_nm),
            (0, 0, movement_nm),
            (0, movement_nm, 0),
        )
    )
    return tuple(
        (dx, dy)
        for _, dx, dy in sorted(set(ranked))
        if dx or dy
    )


def detailed_failure_trials(
    board: PhysicalBoard,
    failed_nets: frozenset[str],
    placement_options: PlacementPlannerOptions,
    movement_nm: int,
    maximum_trials: int,
) -> tuple[PhysicalBoard, ...]:
    """Legal, deterministic perturbations around exact-routing failures."""

    if movement_nm <= 0 or maximum_trials < 0:
        raise ValueError("detailed feedback movement/trial bounds are invalid")
    if board.tracks or board.vias:
        return ()  # Never move a component under accepted copper.
    fixed = set(placement_options.fixed_references) | {
        rule.reference for rule in board.placement_rules
        if rule.fixed_position is not None
    }
    affected = sorted({
        pad.component for net in board.nets if net.name in failed_nets
        for pad in net.pads
    } - fixed)
    placements = {item.reference: item for item in board.placements}
    trials: list[PhysicalBoard] = []
    for reference in affected:
        current = placements[reference]
        for dx, dy in ((movement_nm, 0), (-movement_nm, 0),
                       (0, movement_nm), (0, -movement_nm)):
            moved = replace(current, position=Point(
                current.position.x_nm + dx, current.position.y_nm + dy,
            ))
            try:
                candidate = move_placement_unit(board, placements, reference, moved)
            except ValueError:
                continue
            if not placement_solution_is_legal(board, candidate, placement_options):
                continue
            trials.append(replace(board, placements=tuple(
                candidate[item.reference] for item in board.placements
            )))
            if len(trials) >= maximum_trials:
                return tuple(trials)
    return tuple(trials)
