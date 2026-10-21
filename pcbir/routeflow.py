"""Transactional placement/global-routing feedback controller."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum

from .physical import PhysicalBoard, Placement, Point, nm_from_mm
from .clusters import move_placement_unit
from .progress import ProgressCallback, emit, placement_progress
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
    power_area_plan: object | None = None
    package_patterns: tuple = ()


def optimize_placement_for_routing(
    board: PhysicalBoard,
    placement_options: PlacementPlannerOptions | None = None,
    router_options: GlobalRouterOptions | None = None,
    feedback_options: PlacementRoutingFeedbackOptions | None = None,
    *, on_progress: ProgressCallback | None = None, power_area_options=None, power_native_verifier=None,
    fanout_options=None, pair_state_limit=None,
) -> PlacementRoutingResult:
    """Place, fully global-route, and transactionally relieve congestion."""

    placement_options = placement_options or PlacementPlannerOptions()
    router_options = router_options or GlobalRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=router_options.tile_size_nm
    )
    progress_kwargs = {"on_progress": on_progress} if on_progress is not None else {}
    candidates = generate_placement_candidates(board, placement_options,
        **(dict(power_area_options=power_area_options,power_native_verifier=power_native_verifier,
                global_router_options=router_options,fanout_options=fanout_options,pair_state_limit=pair_state_limit)
           if power_area_options is not None and placement_options.support_pattern_trials else {}),
        **({"progress": placement_progress(on_progress)} if on_progress is not None else {}))
    evaluated = [
        _evaluate_candidate(board, candidate, router_options, **progress_kwargs)
        for candidate in candidates
    ]
    # Provisional geometry is evaluated on each complete legal placement, never
    # installed in the mutable source. Discarding a trial discards its plan too.
    plans = {}
    def power_plan(placed):
        if power_area_options is None:
            return None
        from .hard_macros import materialize_hard_macros
        from .power_geometry import plan_power_areas
        key = id(placed)
        if key not in plans:
            plans[key] = (placed, plan_power_areas(materialize_hard_macros(placed), power_area_options,
                                                  on_progress=on_progress))
        return plans[key][1]
    def power_score(placed):
        plan = power_plan(placed)
        return plan.score[:2] if plan is not None else ()
    for _, placed, _ in evaluated:
        power_plan(placed)
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
                *power_score(item[1]),
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
        if accepted_route.status is GlobalRoutingStatus.SUCCESS and not (power_score(accepted_board) or (0,))[0]:
            break
        current_score = (*power_score(accepted_board), *_route_score(accepted_route))
        best_trial: tuple[tuple[int, ...], PhysicalBoard, GlobalRoutingResult] | None = None
        trials = 0
        for trial_board in _movement_trials(
            accepted_board,
            accepted_route,
            placement_options,
            movement,
            **({"extra_nets": frozenset(o.net for o in power_plan(accepted_board).obligations
                if o.net not in {c.allocation.domain for c in power_plan(accepted_board).selected})
                } if power_area_options is not None else {}),
        ):
            emit(on_progress, "placement_global_trial", "started", iteration=iteration, trial=trials + 1)
            trial_route = route_global(trial_board, router_options, **progress_kwargs)
            emit(on_progress, "placement_global_trial", "finished", iteration=iteration, trial=trials + 1,
                 global_status=trial_route.status.value)
            trials += 1
            score = (*power_score(trial_board), *_route_score(trial_route))
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

    emit(on_progress, "placement_global_certification", "started", candidate=candidate.candidate_id)
    certified = route_global(accepted_board, router_options, **progress_kwargs)
    emit(on_progress, "placement_global_certification", "finished", candidate=candidate.candidate_id,
         global_status=certified.status.value)
    full_route_certified = certified.status is GlobalRoutingStatus.SUCCESS
    return PlacementRoutingResult(
        FeedbackStatus.PASS if full_route_certified else FeedbackStatus.WARNING,
        accepted_board,
        certified,
        candidate.candidate_id,
        tuple(records),
        accepted_moves,
        full_route_certified,
        power_plan(accepted_board),
        candidate.statistics.package_patterns,
    )


def _evaluate_candidate(
    board: PhysicalBoard,
    candidate: PlacementCandidate,
    router_options: GlobalRouterOptions,
    *, on_progress: ProgressCallback | None = None,
) -> tuple[PlacementCandidate, PhysicalBoard, GlobalRoutingResult]:
    placed = replace(board, placements=candidate.placements)
    emit(on_progress, "placement_candidate_global", "started", candidate=candidate.candidate_id, seed=candidate.seed)
    routed = route_global(placed, router_options,
                          **({"on_progress": on_progress} if on_progress is not None else {}))
    emit(on_progress, "placement_candidate_global", "finished", candidate=candidate.candidate_id,
         seed=candidate.seed, global_status=routed.status.value)
    return candidate, placed, routed


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
    *, extra_nets: frozenset[str] = frozenset(),
) -> tuple[PhysicalBoard, ...]:
    if board.tracks or board.vias or board.polygons or board.zone_fills or board.materialized_macros:
        return ()  # Never displace terminals beneath accepted copper.
    routes_by_net = {item.net: item for item in route.routes}
    pressured_nets = {
        net
        for hotspot in route.hotspots
        for net in hotspot.contributors
    } | {net for net, item in routes_by_net.items() if not item.connected} | set(extra_nets)
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
