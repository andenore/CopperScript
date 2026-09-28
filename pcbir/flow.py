"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport, run_physical_drc
from .fanout import FanoutOptions, FanoutResult, route_fanout
from .physical import PhysicalBoard, nm_from_mm
from .placement import PlacementPlannerOptions
from .routeflow import (PlacementRoutingFeedbackOptions, PlacementRoutingResult,
                        detailed_failure_trials, optimize_placement_for_routing)
from .routing import GlobalRouterOptions, GlobalRoutingStatus, route_global


class PhysicalFlowStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"


@dataclass(frozen=True, slots=True)
class RoutingPipelineResult:
    status: PhysicalFlowStatus
    placement_and_global: PlacementRoutingResult
    critical: CriticalRoutingResult
    detailed: DetailedRoutingResult
    drc: PhysicalDrcReport
    fanout: FanoutResult | None = None
    detailed_feedback_trials: int = 0

    @property
    def board(self) -> PhysicalBoard:
        return self.detailed.board


def run_routing_pipeline(
    board: PhysicalBoard,
    *,
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    feedback_options: PlacementRoutingFeedbackOptions | None = None,
    detailed_options: DetailedRouterOptions | None = None,
    drc_policy: PhysicalDrcPolicy | None = None,
    fanout_options: FanoutOptions | None = None,
    detailed_feedback_trials: int = 0,
    detailed_feedback_movement_nm: int = nm_from_mm("0.5"),
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order without weakening an earlier gate."""

    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    if detailed_feedback_trials < 0 or detailed_feedback_movement_nm <= 0:
        raise ValueError("detailed feedback bounds are invalid")
    placement = optimize_placement_for_routing(
        board, placement_options, global_options, feedback_options
    )
    critical = route_critical_nets(placement.board, placement.global_route)
    fanout = route_fanout(critical.board, fanout_options) if fanout_options else None
    detailed = route_detailed(
        fanout.board if fanout else critical.board, placement.global_route,
        detailed_options, fanout_accesses=fanout.accesses if fanout else None,
    )
    drc = run_physical_drc(detailed.board, policy=drc_policy)
    trials_run = 0
    failed_nets = frozenset(item.net for item in detailed.nets if not item.connected)
    if failed_nets and detailed_feedback_trials:
        accepted_score = _detailed_score(detailed, drc)
        for trial_board in detailed_failure_trials(
            placement.board, failed_nets, placement_options,
            detailed_feedback_movement_nm, detailed_feedback_trials,
        ):
            trials_run += 1
            trial_global = route_global(trial_board, global_options)
            if trial_global.status is not GlobalRoutingStatus.SUCCESS:
                continue
            trial_critical = route_critical_nets(trial_board, trial_global)
            if trial_critical.status is CriticalRoutingStatus.FAILED:
                continue
            trial_fanout = (route_fanout(trial_critical.board, fanout_options)
                            if fanout_options else None)
            trial_detailed = route_detailed(
                trial_fanout.board if trial_fanout else trial_critical.board,
                trial_global, detailed_options,
                fanout_accesses=trial_fanout.accesses if trial_fanout else None,
            )
            trial_drc = run_physical_drc(trial_detailed.board, policy=drc_policy)
            score = _detailed_score(trial_detailed, trial_drc)
            if score < accepted_score:
                accepted_score = score
                placement = replace(placement, board=trial_board,
                                    global_route=trial_global,
                                    placement_candidate=placement.placement_candidate + "-detail")
                critical, fanout, detailed, drc = (
                    trial_critical, trial_fanout, trial_detailed, trial_drc,
                )
    passed = (
        placement.full_route_certified
        and critical.status is not CriticalRoutingStatus.FAILED
        and detailed.status is DetailedRoutingStatus.SUCCESS
        and drc.decision is DrcDecision.PASS
    )
    return RoutingPipelineResult(
        PhysicalFlowStatus.PASS if passed else PhysicalFlowStatus.FAIL,
        placement,
        critical,
        detailed,
        drc,
        fanout,
        trials_run,
    )


def _detailed_score(
    detailed: DetailedRoutingResult, drc: PhysicalDrcReport,
) -> tuple[int, ...]:
    hard = sum(item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
               and item.severity.value == "error" for item in drc.findings)
    return (hard, detailed.metrics.unrouted_net_count,
            detailed.metrics.total_conflict_overflow,
            detailed.metrics.via_count, detailed.metrics.total_length_nm)
