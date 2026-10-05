"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .critical_feedback import CriticalPlacementFeedbackResult, improve_critical_placement
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport, run_physical_drc
from .fanout import FanoutOptions, FanoutResult
from .boundary_access import reserve_boundary_access
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .pad_stitch import DuplicatePadStitchResult
from .route_closure import close_detailed_lands
from .physical import PhysicalBoard, nm_from_mm
from .placement import PlacementPlannerOptions
from .routeflow import (FeedbackStatus, PlacementRoutingFeedbackOptions, PlacementRoutingResult,
                        detailed_failure_trials, optimize_placement_for_routing)
from .routing import GlobalRouterOptions, GlobalRoutingStatus, route_global
from .package_access import (PackageAccessOptions, PackageAccessResult, blocked_area_result,
                             improve_package_access, preflight_package_access)
from .progress import ProgressCallback, critical_progress, emit


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
    critical_feedback: CriticalPlacementFeedbackResult | None = None
    package_access: PackageAccessResult | None = None

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
    critical_feedback_trials: int = 0,
    package_access_options: PackageAccessOptions | None = None,
    on_progress: ProgressCallback | None = None,
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order without weakening an earlier gate."""

    from .hard_macros import macro_source
    board = macro_source(board)
    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    detailed_options = detailed_options or DetailedRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    if detailed_feedback_trials < 0 or critical_feedback_trials < 0 or detailed_feedback_movement_nm <= 0:
        raise ValueError("routing placement feedback bounds are invalid")
    emit(on_progress, "placement_global", "started")
    placement = optimize_placement_for_routing(
        board, placement_options, global_options, feedback_options
    )
    emit(on_progress, "placement_global", "finished", candidate=placement.placement_candidate,
         global_status=placement.global_route.status.value)
    access = None
    if fanout_options is not None:
        access_options = package_access_options or PackageAccessOptions()
        access = preflight_package_access(placement.board, placement.global_route,
                                         fanout_options, plane_stitch_options,
                                         options=access_options, on_progress=on_progress)
        # With fanout enabled, critical failures belong to the same escape-first
        # placement transaction, not a second critical-first controller.
        access_options = replace(access_options,
            maximum_trials=access_options.maximum_trials + critical_feedback_trials)
        access = improve_package_access(access, fanout_options,
            options=access_options, placement_options=placement_options,
            global_options=global_options, plane_options=plane_stitch_options, on_progress=on_progress)
        placement = replace(placement, board=access.source, global_route=access.global_route,
            status=FeedbackStatus.PASS if access.global_route.status is GlobalRoutingStatus.SUCCESS else FeedbackStatus.WARNING,
            full_route_certified=access.global_route.status is GlobalRoutingStatus.SUCCESS,
            placement_candidate=placement.placement_candidate + ("-access" if access.accepted_moves else ""))
        if not access.ready:
            emit(on_progress, "ordinary_area", "blocked", reason="package_access_incomplete")
            detailed = blocked_area_result(access)
            return RoutingPipelineResult(PhysicalFlowStatus.FAIL, placement, access.critical,
                detailed, run_physical_drc(detailed.board, policy=drc_policy),
                fanout=replace(access.fanout, board=access.board), plane_stitch=access.plane_stitch,
                package_access=access)
        critical = access.critical
    else:
        critical = route_critical_nets(placement.board, placement.global_route,
                                       on_progress=critical_progress(on_progress))
    critical_feedback = None
    if critical_feedback_trials and access is None:
        critical_feedback = improve_critical_placement(
            placement.board, placement.global_route, critical, maximum_trials=critical_feedback_trials,
            placement_options=placement_options, global_options=global_options,
        )
        critical = critical_feedback.critical
        if critical_feedback.accepted_moves:
            placement = replace(placement, board=critical_feedback.board,
                                global_route=critical_feedback.global_route,
                                status=FeedbackStatus.PASS,
                                full_route_certified=True,
                                placement_candidate=placement.placement_candidate + "-critical")
    plane_stitch = access.plane_stitch if access else (
        stitch_zone_pads(critical.board, plane_stitch_options)
        if plane_stitch_options else None
    )
    pre_fanout = plane_stitch.board if plane_stitch else critical.board
    if access:
        emit(on_progress, "package_boundary_reservation", "started")
    fanout = reserve_boundary_access(pre_fanout, access.fanout, access.boundary) if access else None
    if access:
        emit(on_progress, "package_boundary_reservation", "finished", anchors=len(fanout.boundary_accesses),
             added_tracks=len(fanout.created_tracks) - len(access.fanout.created_tracks))
    emit(on_progress, "ordinary_area", "started")
    detailed = route_detailed(
        fanout.board if fanout else pre_fanout, placement.global_route,
        detailed_options, fanout_accesses=fanout.routing_accesses if fanout else None,
        fanout_created_vias=frozenset((item.net, item.position)
                                    for item in fanout.created_vias) if fanout else None,
        fanout_created_tracks=fanout.created_tracks if fanout else None,
        on_progress=on_progress,
    )
    emit(on_progress, "ordinary_area", "finished", status=detailed.status.value,
         failed_nets=[n.net for n in detailed.nets if not n.connected],
         passes=detailed.metrics.passes)
    emit(on_progress, "land_closure_native_drc", "started")
    detailed, drc, duplicate_stitch = close_detailed_lands(detailed, drc_policy)
    emit(on_progress, "land_closure_native_drc", "finished", decision=drc.decision.value)
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
            emit(on_progress, "detailed_placement_trial", "started", index=trials_run)
            trial_global = route_global(trial_board, global_options)
            if trial_global.status is not GlobalRoutingStatus.SUCCESS:
                emit(on_progress, "detailed_placement_trial", "finished", index=trials_run,
                     outcome="global_failed")
                continue
            trial_access = (preflight_package_access(trial_board, trial_global, fanout_options,
                                                     plane_stitch_options, options=package_access_options,
                                                     on_progress=on_progress) if fanout_options else None)
            if trial_access is not None and not trial_access.ready:
                emit(on_progress, "detailed_placement_trial", "finished", index=trials_run,
                     outcome="package_access_failed")
                continue
            trial_critical = trial_access.critical if trial_access else route_critical_nets(trial_board, trial_global)
            if trial_critical.status is CriticalRoutingStatus.FAILED:
                emit(on_progress, "detailed_placement_trial", "finished", index=trials_run,
                     outcome="critical_failed")
                continue
            trial_plane_stitch = trial_access.plane_stitch if trial_access else (
                stitch_zone_pads(trial_critical.board, plane_stitch_options)
                if plane_stitch_options else None
            )
            trial_pre_fanout = (
                trial_plane_stitch.board if trial_plane_stitch else trial_critical.board
            )
            if trial_access:
                emit(on_progress, "package_boundary_reservation", "started", trial=trials_run)
            trial_fanout = reserve_boundary_access(trial_pre_fanout, trial_access.fanout,
                                                  trial_access.boundary) if trial_access else None
            if trial_access:
                emit(on_progress, "package_boundary_reservation", "finished", trial=trials_run,
                     anchors=len(trial_fanout.boundary_accesses),
                     added_tracks=len(trial_fanout.created_tracks) - len(trial_access.fanout.created_tracks))
            trial_detailed = route_detailed(
                trial_fanout.board if trial_fanout else trial_pre_fanout,
                trial_global, detailed_options,
                fanout_accesses=trial_fanout.routing_accesses if trial_fanout else None,
                fanout_created_vias=frozenset((item.net, item.position)
                                            for item in trial_fanout.created_vias)
                if trial_fanout else None,
                fanout_created_tracks=trial_fanout.created_tracks if trial_fanout else None,
                on_progress=on_progress,
            )
            trial_detailed, trial_drc, trial_duplicate = close_detailed_lands(
                trial_detailed, drc_policy)
            score = _detailed_score(trial_detailed, trial_drc)
            emit(on_progress, "detailed_placement_trial", "finished", index=trials_run,
                 outcome="accepted" if score < accepted_score else "not_improved")
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
                access = trial_access
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
        critical_feedback=critical_feedback,
        package_access=access,
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
