"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport
from .fanout import FanoutOptions, FanoutResult, route_fanout
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .pad_stitch import DuplicatePadStitchResult
from .route_closure import close_detailed_lands
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
    plane_stitch: PlaneStitchResult | None = None
    detailed_feedback_trials: int = 0
    duplicate_pad_stitch: DuplicatePadStitchResult | None = None

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
    plane_stitch_options: PlaneStitchOptions | None = None,
    detailed_feedback_trials: int = 0,
    detailed_feedback_movement_nm: int = nm_from_mm("0.5"),
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order without weakening an earlier gate."""

    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    detailed_options = detailed_options or DetailedRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    if detailed_feedback_trials < 0 or detailed_feedback_movement_nm <= 0:
        raise ValueError("detailed feedback bounds are invalid")
    placement = optimize_placement_for_routing(
        board, placement_options, global_options, feedback_options
    )
    critical = route_critical_nets(placement.board, placement.global_route)
    plane_stitch = (
        stitch_zone_pads(critical.board, plane_stitch_options)
        if plane_stitch_options else None
    )
    pre_fanout = plane_stitch.board if plane_stitch else critical.board
    fanout = route_fanout(pre_fanout, fanout_options) if fanout_options else None
    detailed = route_detailed(
        fanout.board if fanout else pre_fanout, placement.global_route,
        detailed_options, fanout_accesses=fanout.accesses if fanout else None,
        fanout_created_vias=frozenset((item.net, item.position)
                                    for item in fanout.created_vias) if fanout else None,
    )
    detailed, drc, duplicate_stitch = close_detailed_lands(detailed, drc_policy)
    trials_run = 0
    # A deferred zone net is awaiting external fill evidence, not a detailed
    # maze-route failure. Moving components to "repair" it cannot improve the
    # detailed search and can displace already-routed signals.
    deferred_zones = (
        {zone.net for zone in board.zones}
        if detailed_options is not None and detailed_options.defer_zone_nets
        else set()
    )
    failed_nets = frozenset(
        item.net for item in detailed.nets
        if not item.connected and item.net not in deferred_zones
    )
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
            trial_plane_stitch = (
                stitch_zone_pads(trial_critical.board, plane_stitch_options)
                if plane_stitch_options else None
            )
            trial_pre_fanout = (
                trial_plane_stitch.board if trial_plane_stitch else trial_critical.board
            )
            trial_fanout = (route_fanout(trial_pre_fanout, fanout_options)
                            if fanout_options else None)
            trial_detailed = route_detailed(
                trial_fanout.board if trial_fanout else trial_pre_fanout,
                trial_global, detailed_options,
                fanout_accesses=trial_fanout.accesses if trial_fanout else None,
                fanout_created_vias=frozenset((item.net, item.position)
                                            for item in trial_fanout.created_vias)
                if trial_fanout else None,
            )
            trial_detailed, trial_drc, trial_duplicate = close_detailed_lands(
                trial_detailed, drc_policy)
            score = _detailed_score(trial_detailed, trial_drc)
            if score < accepted_score:
                accepted_score = score
                placement = replace(placement, board=trial_board,
                                    global_route=trial_global,
                                    placement_candidate=placement.placement_candidate + "-detail")
                critical, plane_stitch, fanout, detailed, drc = (
                    trial_critical, trial_plane_stitch, trial_fanout,
                    trial_detailed, trial_drc,
                )
                duplicate_stitch = trial_duplicate
    passed = (
        placement.full_route_certified
        and critical.status is not CriticalRoutingStatus.FAILED
        and detailed.status is DetailedRoutingStatus.SUCCESS
        and drc.decision is DrcDecision.PASS
    )
    return RoutingPipelineResult(
        status=PhysicalFlowStatus.PASS if passed else PhysicalFlowStatus.FAIL,
        placement_and_global=placement,
        critical=critical,
        detailed=detailed,
        drc=drc,
        fanout=fanout,
        plane_stitch=plane_stitch,
        detailed_feedback_trials=trials_run,
        duplicate_pad_stitch=duplicate_stitch,
    )


def _detailed_score(
    detailed: DetailedRoutingResult, drc: PhysicalDrcReport,
) -> tuple[int, ...]:
    hard = sum(item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
               and item.severity.value == "error" for item in drc.findings)
    zone_nets = {zone.net for zone in detailed.board.zones}
    actual_opens = {net for item in drc.findings if item.code == "DRC-OPEN-NET"
                    for net in item.nets if net not in zone_nets}
    return (hard, len(actual_opens), detailed.metrics.unrouted_net_count,
            detailed.metrics.total_conflict_overflow,
            detailed.metrics.via_count, detailed.metrics.total_length_nm)
