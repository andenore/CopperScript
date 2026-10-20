"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

from dataclasses import dataclass, field, replace
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
    power_area_plan: object | None = None
    power_area_status: str = "disabled"
    power_area_options: object | None = None
    power_preparation: object | None = None
    power_preparation_validation: object | None = None
    power_native_verifier: object | None = field(default=None, repr=False, compare=False)

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
    power_area_options=None,
    power_native_verifier=None,
    routing_nets: frozenset[str] | None = None,
    on_progress: ProgressCallback | None = None,
    legacy_gates: bool = False,
    unified_options=None,
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order and always reach ordinary routing.

    An incomplete package access or a rejected power preparation is reported,
    and ordinary routing then works around whatever access exists. With
    ``legacy_gates`` the pre-refactor behaviour returns early instead.
    With ``unified_options`` the unified negotiated router replaces the
    ordinary detailed router.
    """
    unified_fanout = None
    if unified_options is not None:
        from functools import partial
        route_ordinary = partial(_route_unified_compat, unified_options,
                                 escape_options=fanout_options)
        # The unified router owns terminal access itself: no package-access
        # search, only deterministic BGA dogbones before ordinary routing.
        # Power comes from declared planes (refactor plan P3), not the
        # power-preparation allocator. The established escape generator still
        # seeds pad launches (once, no gate, no search over alternatives).
        unified_fanout = fanout_options
        fanout_options = None
        if power_area_options is not None:
            emit(on_progress, "power_area_planning", "skipped", reason="unified router uses declared planes")
        power_area_options = None
    else:
        route_ordinary = route_detailed

    from .hard_macros import macro_source
    board = macro_source(board)
    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    detailed_options = detailed_options or DetailedRouterOptions()
    package_access_options = package_access_options or PackageAccessOptions()
    if routing_nets is not None:
        if (power_area_options is None or not power_area_options.plane_return_nets
                or not isinstance(routing_nets, frozenset)
                or not routing_nets <= {n.name for n in board.nets}):
            raise ValueError('scoped routing requires native return preparation and declared net identities')
    if (fanout_options is not None and plane_stitch_options is None
            and package_access_options.reserve_plane_contacts and board.zones):
        plane_stitch_options = PlaneStitchOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    if detailed_feedback_trials < 0 or critical_feedback_trials < 0 or detailed_feedback_movement_nm <= 0:
        raise ValueError("routing placement feedback bounds are invalid")
    emit(on_progress, "placement_global", "started")
    placement = optimize_placement_for_routing(
        board, placement_options, global_options, feedback_options,
        **({"power_area_options": power_area_options} if power_area_options is not None else {}),
        **({"power_native_verifier": power_native_verifier,"fanout_options":fanout_options,
            "pair_state_limit":package_access_options.initial_pair_state_limit or None}
           if power_area_options is not None and placement_options.support_pattern_trials else {}),
        **({"on_progress": on_progress} if on_progress is not None else {})
    )
    emit(on_progress, "placement_global", "finished", candidate=placement.placement_candidate,
         global_status=placement.global_route.status.value)
    access = None
    preparation, preparation_validation = None, None
    if power_area_options is not None and power_area_options.plane_return_nets:
        from .power_preparation import prepare_power_distribution
        from .package_access import preflight_prepared_power_access
        from .fanout import route_fanout
        if placement.package_patterns:
            offsets=dict(power_area_options.package_pattern_offsets)
            offsets.update((r['component'],r['pattern']['offset_nm']) for r in placement.package_patterns
                           if r.get('pattern') is not None and r.get('status')=='complete_geometry_proposal')
            # These are search seeds only. The exact post-critical scene must
            # freshly accept every contact; changed poses never reuse evidence.
            power_area_options=replace(power_area_options,package_patterns_before_contacts=True,
                                       package_pattern_offsets=tuple(sorted(offsets.items())))
        if fanout_options is None:
            raise ValueError('native plane-return preparation requires package fanout')
        fanout_options = replace(fanout_options,
            plane_return_nets=frozenset(power_area_options.plane_return_nets))
        if power_area_options.reserve_ordinary_before_contacts:
            fanout_options = replace(fanout_options, prefer_surface_escapes=True, surface_fallback=True,
                surface_preference_depth=1 if fanout_options.surface_preference_depth is None
                                          else fanout_options.surface_preference_depth)
        preparation = prepare_power_distribution(placement.board, placement.global_route,
            power_area_options, fanout_options, native_verifier=power_native_verifier,
            pair_state_limit=package_access_options.initial_pair_state_limit or None,
            on_progress=on_progress, ordinary_nets=routing_nets)
        if preparation.accepted:
            access = preflight_prepared_power_access(placement.board, placement.global_route,
                preparation, fanout_options, plane_stitch_options,
                options=package_access_options, on_progress=on_progress, only_nets=routing_nets)
        else:
            # Diagnostic critical copper is retained; no rejected power/contact
            # contribution or connectivity flag is promoted into the pipeline.
            access = PackageAccessResult(placement.board, placement.global_route,
                FanoutResult(preparation.source, {}, (), 0, 0), preparation.critical,
                None, frozenset(), 0)
    if fanout_options is not None:
        access_options = package_access_options or PackageAccessOptions()
        if preparation is None:
            access = preflight_package_access(placement.board, placement.global_route,
                                             fanout_options, plane_stitch_options,
                                             options=access_options, on_progress=on_progress)
        # With fanout enabled, critical failures belong to the same escape-first
        # placement transaction, not a second critical-first controller.
        access_options = replace(access_options,
            maximum_trials=access_options.maximum_trials + critical_feedback_trials)
        if preparation is None:
            access = improve_package_access(access, fanout_options,
                options=access_options, placement_options=placement_options,
                global_options=global_options, plane_options=plane_stitch_options, on_progress=on_progress)
        placement = replace(placement, board=access.source, global_route=access.global_route,
            status=FeedbackStatus.PASS if access.global_route.status is GlobalRoutingStatus.SUCCESS else FeedbackStatus.WARNING,
            full_route_certified=access.global_route.status is GlobalRoutingStatus.SUCCESS,
            placement_candidate=placement.placement_candidate + ("-access" if access.accepted_moves else ""))
        if not access.ready and not legacy_gates:
            emit(on_progress, "package_access", "incomplete", pending_pads=len(access.pending_pads))
        elif not access.ready:
            emit(on_progress, "ordinary_area", "blocked", reason="package_access_incomplete")
            detailed = blocked_area_result(access)
            return RoutingPipelineResult(PhysicalFlowStatus.FAIL, placement, access.critical,
                detailed, run_physical_drc(detailed.board, policy=drc_policy),
                fanout=replace(access.fanout, board=access.board), plane_stitch=access.plane_stitch,
                package_access=access, power_area_plan=preparation.plan if preparation else placement.power_area_plan,
                power_area_status=(preparation.status if preparation and not preparation.accepted else
                                   "package_access_incomplete") if power_area_options is not None else "disabled",
                power_area_options=power_area_options, power_preparation=preparation,
                power_native_verifier=power_native_verifier)
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
    if access is None and critical.board.decoupling_links:
        from .decoupling import route_decouplers
        critical = replace(critical, board=route_decouplers(critical.board).board)
    if unified_options is not None and getattr(unified_options, "bga_fanout", True):
        from .bga_fanout import plan_bga_fanouts
        emit(on_progress, "bga_fanout", "started")
        bga = plan_bga_fanouts(critical.board)
        critical = replace(critical, board=bga.board)
        emit(on_progress, "bga_fanout", "finished", **bga.report())
    plane_stitch = access.plane_stitch if access else (
        stitch_zone_pads(critical.board, plane_stitch_options)
        if plane_stitch_options else None
    )
    pre_fanout = plane_stitch.board if plane_stitch else critical.board
    power_plan = preparation.plan if preparation else None
    power_status = 'contact_routed_native_pending' if preparation else 'disabled'
    def plan_distribution(source, held_fanout=None):
        from .power_geometry import plan_power_areas, apply_power_area_plan
        emit(on_progress, "power_area_planning", "started")
        plan = plan_power_areas(source, power_area_options, on_progress=on_progress)
        candidate, status = apply_power_area_plan(source, plan)
        if candidate is not source and held_fanout is not None:
            from .pin_escape import RoutingAccess, verified_routing_access, verified_fanout_path
            from .routing_clearance import RoutingClearanceIndex
            clearance = RoutingClearanceIndex(candidate)
            net_by_pad = {p: n.name for n in candidate.nets for p in n.pads}
            for pad, anchor in held_fanout.routing_accesses.items():
                witness = (verified_routing_access(candidate, pad, net_by_pad[pad], anchor, clearance)
                    if isinstance(anchor, RoutingAccess) else
                    verified_fanout_path(candidate, pad, net_by_pad[pad], anchor, clearance))
                if witness is None:
                    candidate, status = source, "held_package_exit_rejected"
                    break
        emit(on_progress, "power_area_planning", "finished", status=status,
             domains=len(plan.obligations), selected=len(plan.selected), exhausted=plan.exhausted)
        return candidate, plan, status
    if access:
        emit(on_progress, "package_boundary_reservation", "started")
    from .zone_geometry import deferred_zone_nets
    deferred_zones = (frozenset(deferred_zone_nets(pre_fanout))
                      if detailed_options.defer_zone_nets else frozenset())
    if access and access.ready:
        fanout = reserve_boundary_access(pre_fanout, access.fanout, access.boundary,
                                         deferred_nets=deferred_zones)
    elif access:
        # Incomplete access: keep the verified launches that exist and let the
        # ordinary router reach the remaining pads itself.
        fanout = replace(access.fanout, board=pre_fanout)
    else:
        fanout = None
    if access:
        emit(on_progress, "package_boundary_reservation", "finished", anchors=len(fanout.boundary_accesses or {}),
             added_tracks=len(fanout.created_tracks) - len(access.fanout.created_tracks))
    pre_detailed = fanout.board if fanout else pre_fanout
    if preparation is not None and not preparation.accepted and not legacy_gates:
        power_status = 'power_preparation_rejected'
        preparation = None
    if preparation is not None:
        from .power_preparation import validate_power_preparation
        evidence = power_native_verifier(pre_detailed, 'power-return-boundary')
        preparation_validation = validate_power_preparation(preparation, pre_detailed, evidence, power_area_options)
        if preparation_validation['status'] != 'native_connected' and not legacy_gates:
            power_status = 'power_preparation_boundary_rejected'
        elif preparation_validation['status'] != 'native_connected':
            restored_fanout = FanoutResult(preparation.board, {}, (), 0, 0)
            blocked = replace(access, boundary=None, fanout=restored_fanout,
                critical=replace(access.critical, board=preparation.board), plane_stitch=None)
            detailed = blocked_area_result(blocked)
            return RoutingPipelineResult(PhysicalFlowStatus.FAIL, placement, blocked.critical,
                detailed, run_physical_drc(detailed.board, policy=drc_policy),
                fanout=blocked.fanout, package_access=blocked, power_area_plan=power_plan,
                power_area_status='power_preparation_boundary_rejected', power_area_options=power_area_options,
                power_preparation=preparation, power_preparation_validation=preparation_validation,
                power_native_verifier=power_native_verifier)
    elif power_area_options is not None:
        pre_detailed, power_plan, power_status = plan_distribution(pre_detailed, fanout)
        deferred_zones = (frozenset(deferred_zone_nets(pre_detailed))
                          if detailed_options.defer_zone_nets else frozenset())
    emit(on_progress, "ordinary_area", "started")
    detailed = route_ordinary(
        pre_detailed, placement.global_route,
        detailed_options, fanout_accesses=fanout.routing_accesses if fanout else None,
        fanout_created_vias=frozenset((item.net, item.position)
                                    for item in fanout.created_vias) if fanout else None,
        fanout_created_tracks=fanout.created_tracks if fanout else None,
        on_progress=on_progress,
        only_nets=routing_nets,
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
    failed_nets = frozenset(
        item.net for item in detailed.nets
        if not item.connected and item.net not in deferred_zones
    )
    if failed_nets and detailed_feedback_trials and preparation is None:
        accepted_score = _detailed_score(detailed, drc)
        for trial_board in detailed_failure_trials(
            placement.board, failed_nets, placement_options,
            detailed_feedback_movement_nm, detailed_feedback_trials,
        ):
            trials_run += 1
            emit(on_progress, "detailed_placement_trial", "started", index=trials_run)
            trial_global = route_global(trial_board, global_options,
                **({"on_progress": on_progress} if on_progress is not None else {}))
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
            trial_power_plan, trial_power_status = None, "disabled"
            if trial_access:
                emit(on_progress, "package_boundary_reservation", "started", trial=trials_run)
            trial_fanout = reserve_boundary_access(trial_pre_fanout, trial_access.fanout,
                trial_access.boundary, deferred_nets=frozenset(deferred_zone_nets(trial_pre_fanout))
                if detailed_options.defer_zone_nets else frozenset()) if trial_access else None
            if trial_access:
                emit(on_progress, "package_boundary_reservation", "finished", trial=trials_run,
                     anchors=len(trial_fanout.boundary_accesses),
                     added_tracks=len(trial_fanout.created_tracks) - len(trial_access.fanout.created_tracks))
            trial_pre_detailed = trial_fanout.board if trial_fanout else trial_pre_fanout
            if power_area_options is not None:
                trial_pre_detailed, trial_power_plan, trial_power_status = plan_distribution(trial_pre_detailed, trial_fanout)
            trial_detailed = route_ordinary(
                trial_pre_detailed,
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
                power_plan, power_status = trial_power_plan, trial_power_status
    if preparation is not None:
        evidence = power_native_verifier(detailed.board, 'power-return-ordinary')
        preparation_validation = validate_power_preparation(preparation, detailed.board, evidence, power_area_options)
        if preparation_validation['status'] != 'native_connected' and not legacy_gates:
            power_status = 'power_preparation_ordinary_rejected'
        elif preparation_validation['status'] != 'native_connected':
            # Roll back the complete ordinary trial, including closure/cleanup.
            blocked = replace(access, boundary=None, plane_stitch=None,
                              fanout=fanout, critical=replace(access.critical, board=pre_detailed))
            detailed = blocked_area_result(blocked)
            drc = run_physical_drc(detailed.board, policy=drc_policy)
            access = blocked
            plane_stitch, duplicate_stitch = None, None
            power_status = 'power_preparation_ordinary_rejected'
    passed = (
        routing_nets is None and placement.full_route_certified
        and critical.status is not CriticalRoutingStatus.FAILED
        and detailed.status is DetailedRoutingStatus.SUCCESS
        and drc.decision is DrcDecision.PASS
        and not (power_plan is not None and power_plan.partitioned_zone_ids)
        and (preparation is None or preparation_validation['status'] == 'native_connected')
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
        power_area_plan=power_plan,
        power_area_status=power_status,
        power_area_options=power_area_options,
        power_preparation=preparation,
        power_preparation_validation=preparation_validation,
        power_native_verifier=power_native_verifier,
    )


def _route_unified_compat(unified_options, board, global_route, _detailed_options, *,
                          escape_options=None, **kwargs):
    from .unified_router import route_unified
    seeder = None
    if escape_options is not None:
        from .fanout import route_fanout

        def seeder(partial, nets):
            return route_fanout(partial, escape_options, only_nets=nets).board
    return route_unified(board, global_route, unified_options, escape_seeder=seeder, **kwargs)


def _detailed_score(
    detailed: DetailedRoutingResult, drc: PhysicalDrcReport,
) -> tuple[int, ...]:
    hard = sum(item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
               and item.severity.value == "error" for item in drc.findings)
    from .zone_geometry import deferred_zone_nets
    zone_nets = deferred_zone_nets(detailed.board)
    actual_opens = {net for item in drc.findings if item.code == "DRC-OPEN-NET"
                    for net in item.nets if net not in zone_nets}
    return (hard, len(actual_opens), detailed.metrics.unrouted_net_count,
            detailed.metrics.total_conflict_overflow,
            detailed.metrics.via_count, detailed.metrics.total_length_nm)
