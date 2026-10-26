"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

import os
from dataclasses import dataclass, field, replace
from enum import Enum
from functools import partial

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .critical_feedback import CriticalPlacementFeedbackResult, improve_critical_placement
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport, run_physical_drc
from .fanout import FanoutOptions, FanoutResult
from .boundary_access import reserve_boundary_access
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .pad_stitch import DuplicatePadStitchResult
from .route_closure import close_detailed_lands
from .physical import PhysicalBoard, RouteKind, nm_from_mm
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


# Stage boundaries where an iteration may stop early (``stop_after``). Stopping
# never passes a gate: the result is a FAIL with ``stopped_after`` set.
STOP_STAGES = ("placement", "package-preflight", "package-access")


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
    stopped_after: str | None = None
    # Retain the selected backend for later transactional dependency repairs.
    unified_options: object | None = None
    # congestion_feedback.FeedbackOutcome of the unified path's placement
    # feedback loop (None when it was not enabled or on the legacy router).
    placement_feedback: object | None = None

    @property
    def board(self) -> PhysicalBoard:
        return self.detailed.board


def _stopped_after_placement(placement: PlacementRoutingResult, drc_policy) -> tuple:
    """Zero-pass critical/detailed results for a run stopped after placement."""

    from .critical import _fingerprint as critical_fingerprint
    from .detailed import DetailedNetResult, DetailedRoutingMetrics, _fingerprint as detailed_fingerprint
    from .hard_macros import materialize_hard_macros
    # A placement preview still owns its prescribed immutable copper and via
    # arrays. Exporting a bare footprint scene would drop that intent. Commit
    # and validate the complete prefix just as the later routing stage does;
    # the held placement checkpoint itself remains untouched.
    board = materialize_hard_macros(placement.board)
    board = replace(board, metadata={**board.metadata, "stopped_after": "placement",
                                               "detailed_routing": "not_run", "fabrication_ready": "false"})
    global_fp = placement.global_route.routing_fingerprint
    critical = CriticalRoutingResult(CriticalRoutingStatus.WARNING, board, (), (), (), global_fp,
                                     critical_fingerprint(global_fp, [], [], []))
    reason = "not routed: run stopped after placement (--stop-after placement)"
    nets = tuple(DetailedNetResult(net.name, False, 0, 0, 0, 0, (reason,))
                 for net in board.nets if len(net.pads) >= 2)
    metrics = DetailedRoutingMetrics(0, len(nets), 0, 0, 0, 0, 0, 0)
    detailed = DetailedRoutingResult(DetailedRoutingStatus.PARTIAL, board, nets, metrics, len(board.tracks),
                                     len(board.vias), global_fp, detailed_fingerprint(global_fp, board, metrics))
    return critical, detailed, run_physical_drc(board, policy=drc_policy)


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
    stop_after: str | None = None,
    initial_placement: PlacementRoutingResult | None = None,
    on_stage=None,
    escape_plan_options=None,
    placement_feedback_trials: int = 0,
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order and always reach ordinary routing.

    An incomplete package access or a rejected power preparation is reported,
    and ordinary routing then works around whatever access exists. With
    ``legacy_gates`` the pre-refactor behaviour returns early instead.
    With ``unified_options`` the unified negotiated router replaces the
    ordinary detailed router.

    ``stop_after`` ends the run at a stage boundary (see ``STOP_STAGES``) with a
    FAIL result carrying ``stopped_after``. ``initial_placement`` replaces the
    placement/global stage with an earlier, explicitly supplied result.
    ``on_stage(name, value)`` observes completed stages (e.g. for checkpoints).
    ``escape_plan_options`` with ``two_end_order`` plans crossing bundles on
    the placed board and gives the plan to escape generation and to the
    ordinary or unified router (escape planning E3, E4).

    Placement feedback: under the legacy router ``critical_feedback_trials``
    and ``detailed_feedback_trials`` drive their own controllers. Under the
    unified router both map onto the congestion-driven loop
    (``congestion_feedback.run_placement_feedback``) with a budget of
    ``max(placement_feedback_trials, detailed + critical)`` full reruns of
    the pipeline tail, started at ``detailed_feedback_movement_nm``.
    """
    unified_fanout = None
    if unified_options is not None:
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
    if package_access_options.guide_replan_options is None:
        package_access_options = replace(package_access_options, guide_replan_options=global_options)
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
    if (detailed_feedback_trials < 0 or critical_feedback_trials < 0 or placement_feedback_trials < 0
            or detailed_feedback_movement_nm <= 0):
        raise ValueError("routing placement feedback bounds are invalid")
    if stop_after is not None and stop_after not in STOP_STAGES:
        raise ValueError(f"stop_after must be one of {', '.join(STOP_STAGES)}")
    if stop_after in {"package-preflight", "package-access"} and unified_options is not None:
        raise ValueError(f"stopping after {stop_after} requires --router legacy "
                         "(the unified router has no package-access stage)")
    if stop_after in {"package-preflight", "package-access"} and fanout_options is None:
        raise ValueError(f"stopping after {stop_after} requires package fanout (--fanout)")
    if initial_placement is not None:
        if (initial_placement.board.name != board.name
                or {n.name for n in initial_placement.board.nets} != {n.name for n in board.nets}):
            raise ValueError("the supplied placement belongs to a different board")
        placement = initial_placement
        emit(on_progress, "placement_global", "reused", candidate=placement.placement_candidate,
             global_status=placement.global_route.status.value)
    else:
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
    if on_stage is not None:
        on_stage("placement", placement)
    if stop_after == "placement":
        emit(on_progress, "ordinary_area", "stopped", reason="stop_after_placement")
        critical, detailed, drc = _stopped_after_placement(placement, drc_policy)
        return RoutingPipelineResult(PhysicalFlowStatus.FAIL, placement, critical, detailed, drc,
                                     power_area_options=power_area_options,
                                     power_area_status="stopped_after_placement" if power_area_options is not None else "disabled",
                                     stopped_after="placement", unified_options=unified_options)
    array_plans, surface_balls = _plan_array_escapes(placement.board, escape_plan_options, on_progress)
    if surface_balls is not None:
        if unified_fanout is not None:
            unified_fanout = replace(unified_fanout, surface_pads=unified_fanout.surface_pads | surface_balls)
            route_ordinary = partial(_route_unified_compat, unified_options, escape_options=unified_fanout)
        elif fanout_options is not None:
            fanout_options = replace(fanout_options, surface_pads=fanout_options.surface_pads | surface_balls)
    lattice = _plan_lattice(placement.board, escape_plan_options, surface_balls, on_progress)
    if escape_plan_options is not None and escape_plan_options.two_end_order:
        # Plan once on the placed board: escape generation keeps kept nets'
        # surface pins off dogbone vias, and the router follows the plan's
        # order and layers (the ordinary router and the unified router alike).
        from .escape_plan import plan_two_end_escapes
        escape_plan = plan_two_end_escapes(placement.board, escape_plan_options)
        emit(on_progress, "escape_plan", "finished", bundles=len(escape_plan.pairs),
             planned_nets=len(escape_plan.net_order), surface_pads=len(escape_plan.surface_pads))
        if unified_options is not None:
            unified_options = replace(unified_options, escape_net_order=escape_plan.net_order,
                                      escape_layer_biases=escape_plan.layer_biases)
            if unified_fanout is not None:
                unified_fanout = replace(unified_fanout,
                                         surface_pads=unified_fanout.surface_pads | escape_plan.surface_pads)
            route_ordinary = partial(_route_unified_compat, unified_options, escape_options=unified_fanout)
        else:
            if fanout_options is not None:
                fanout_options = replace(fanout_options,
                                         surface_pads=fanout_options.surface_pads | escape_plan.surface_pads)
            detailed_options = replace(detailed_options, escape_net_order=escape_plan.net_order,
                                       escape_layer_biases=escape_plan.layer_biases)
    access = None
    preparation, preparation_validation = None, None
    stopped = None
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
            if stop_after in {"package-preflight", "package-access"}:
                stopped = stop_after
        else:
            # Diagnostic critical copper is retained; no rejected power/contact
            # contribution or connectivity flag is promoted into the pipeline.
            access = PackageAccessResult(placement.board, placement.global_route,
                FanoutResult(preparation.source, {}, (), 0, 0), preparation.critical,
                None, frozenset(), 0)
    reserved = held = None
    held_as_copper = False
    if unified_options is not None and getattr(unified_options, "direct", False):
        critical = _direct_critical(placement)
        plane_stitch_options = None
        emit(on_progress, "unified_direct", "started",
             reason="all mutable nets negotiate together; only source-fixed copper is retained")
    elif fanout_options is not None:
        access_options = package_access_options or PackageAccessOptions()
        if preparation is None:
            access = preflight_package_access(placement.board, placement.global_route,
                                             fanout_options, plane_stitch_options,
                                             options=access_options, on_progress=on_progress,
                                             on_checkpoint=on_stage)
            if stop_after == "package-preflight":
                stopped = "package-preflight"
        # With fanout enabled, critical failures belong to the same escape-first
        # placement transaction, not a second critical-first controller.
        access_options = replace(access_options,
            maximum_trials=access_options.maximum_trials + critical_feedback_trials)
        if preparation is None and stopped is None:
            access = improve_package_access(access, fanout_options,
                options=access_options, placement_options=placement_options,
                global_options=global_options, plane_options=plane_stitch_options, on_progress=on_progress)
            if stop_after == "package-access":
                stopped = "package-access"
        placement = replace(placement, board=access.source, global_route=access.global_route,
            status=FeedbackStatus.PASS if access.global_route.status is GlobalRoutingStatus.SUCCESS else FeedbackStatus.WARNING,
            full_route_certified=access.global_route.status is GlobalRoutingStatus.SUCCESS,
            placement_candidate=placement.placement_candidate + ("-access" if access.accepted_moves else ""))
        if on_stage is not None:
            on_stage(stopped or "package-access", access)
        if stopped is not None or (not access.ready and legacy_gates):
            if stopped is not None:
                emit(on_progress, "ordinary_area", "stopped", reason=f"stop_after_{stopped}",
                     package_access_ready=access.ready)
            else:
                emit(on_progress, "ordinary_area", "blocked", reason="package_access_incomplete")
            detailed = blocked_area_result(access, stopped_after=stopped)
            return RoutingPipelineResult(PhysicalFlowStatus.FAIL, placement, access.critical,
                detailed, run_physical_drc(detailed.board, policy=drc_policy),
                fanout=replace(access.fanout, board=access.board), plane_stitch=access.plane_stitch,
                package_access=access, power_area_plan=preparation.plan if preparation else placement.power_area_plan,
                power_area_status=(f"stopped_after_{stopped}" if stopped is not None else
                                   preparation.status if preparation and not preparation.accepted else
                                   "package_access_incomplete") if power_area_options is not None else "disabled",
                power_area_options=power_area_options, power_preparation=preparation,
                power_native_verifier=power_native_verifier, stopped_after=stopped, unified_options=unified_options)
        if not access.ready:
            emit(on_progress, "package_access", "incomplete", pending_pads=len(access.pending_pads))
        critical = access.critical
    else:
        critical, reserved, held, held_as_copper = _route_critical_stage(
            placement, unified_options=unified_options, unified_fanout=unified_fanout,
            package_access_options=package_access_options, plane_stitch_options=plane_stitch_options,
            on_progress=on_progress, on_stage=on_stage, lattice=lattice)
    critical_feedback = None
    if critical_feedback_trials and access is None and unified_options is None:
        critical_feedback = improve_critical_placement(
            placement.board, placement.global_route, critical, maximum_trials=critical_feedback_trials,
            placement_options=placement_options, global_options=global_options, unified_options=unified_options,
        )
        critical = critical_feedback.critical
        if critical_feedback.accepted_moves:
            placement = replace(placement, board=critical_feedback.board,
                                global_route=critical_feedback.global_route,
                                status=FeedbackStatus.PASS,
                                full_route_certified=True,
                                placement_candidate=placement.placement_candidate + "-critical")
    if lattice is not None and critical_feedback is not None and critical_feedback.accepted_moves:
        lattice = None  # placement moved: the lattice no longer applies
        critical = replace(critical, board=replace(critical.board, copper_keepouts=tuple(
            k for k in critical.board.copper_keepouts if not k.id.startswith("lattice-reserve"))))
    if held is not None and critical_feedback is not None and critical_feedback.accepted_moves:
        held = None  # placement moved: the planned bypass copper no longer applies
        critical = replace(critical, board=replace(critical.board, copper_keepouts=tuple(
            k for k in critical.board.copper_keepouts if not k.id.startswith("bypass-reserve"))))
    critical, plane_stitch = _complete_critical_stage(
        critical, reserved, held, held_as_copper, access=access, unified_options=unified_options,
        plane_stitch_options=plane_stitch_options, on_progress=on_progress,
        lattice=lattice, array_plans=array_plans, escape_plan_options=escape_plan_options,
        save_inputs_path=getattr(unified_options, "save_inputs_path", None), fanout_options=unified_fanout)
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
                power_native_verifier=power_native_verifier, unified_options=unified_options)
    elif power_area_options is not None:
        pre_detailed, power_plan, power_status = plan_distribution(pre_detailed, fanout)
        deferred_zones = (frozenset(deferred_zone_nets(pre_detailed))
                          if detailed_options.defer_zone_nets else frozenset())
    critical, plane_stitch, fanout, pre_detailed = _release_escape_masks(
        reserved, critical, plane_stitch, fanout, pre_detailed, unified_options=unified_options,
        on_progress=on_progress)
    detailed, drc, duplicate_stitch = _route_ordinary_stage(
        route_ordinary, pre_detailed, placement.global_route, detailed_options, fanout,
        drc_policy=drc_policy, routing_nets=routing_nets, on_progress=on_progress)
    trials_run = 0
    # A deferred zone net is awaiting external fill evidence, not a detailed
    # maze-route failure. Moving components to "repair" it cannot improve the
    # detailed search and can displace already-routed signals.
    failed_nets = frozenset(
        item.net for item in detailed.nets
        if not item.connected and item.net not in deferred_zones
    )
    placement_feedback = None
    feedback_budget = (max(placement_feedback_trials, detailed_feedback_trials + critical_feedback_trials)
                       if unified_options is not None else 0)
    if feedback_budget and preparation is None and routing_nets is None:
        from .congestion_feedback import FeedbackState, run_placement_feedback
        # Each trial routes from its own placement, so it must not overwrite
        # the replay inputs of the run that is kept: trials save beside them,
        # and only the accepted trial's file replaces them (see below).
        inputs_path = getattr(unified_options, "save_inputs_path", None)
        trial_inputs: list[str] = []

        def rerun_tail(trial_board, trial_global):
            if inputs_path:
                trial_inputs.append(f"{inputs_path}.trial-{len(trial_inputs) + 1}")
                trial_route = partial(_route_unified_compat,
                                      replace(unified_options, save_inputs_path=trial_inputs[-1]),
                                      escape_options=unified_fanout)
            else:
                trial_route = route_ordinary
            # Exactly the main path's tail on the unified router, from the
            # trial placement: reservations, critical routing, held copper,
            # islands, dogbones, plane stitch, ordinary routing and closure.
            trial = replace(placement, board=trial_board, global_route=trial_global)
            # The escape frame (array plans, via lattice) of the trial placement.
            trial_plans, trial_surface = _plan_array_escapes(trial_board, escape_plan_options, on_progress)
            trial_lattice = _plan_lattice(trial_board, escape_plan_options, trial_surface, on_progress)
            if getattr(unified_options, "direct", False):
                trial_critical, trial_reserved, trial_held, trial_as_copper = (
                    _direct_critical(trial), None, None, False)
            else:
                trial_critical, trial_reserved, trial_held, trial_as_copper = _route_critical_stage(
                    trial, unified_options=unified_options, unified_fanout=unified_fanout,
                    package_access_options=package_access_options, on_progress=on_progress, on_stage=None,
                    plane_stitch_options=plane_stitch_options, lattice=trial_lattice)
            trial_critical, trial_stitch = _complete_critical_stage(
                trial_critical, trial_reserved, trial_held, trial_as_copper, access=None,
                unified_options=unified_options, plane_stitch_options=plane_stitch_options,
                on_progress=on_progress, lattice=trial_lattice, array_plans=trial_plans,
                escape_plan_options=escape_plan_options,
                save_inputs_path=trial_inputs[-1] if inputs_path else None, fanout_options=unified_fanout)
            trial_pre = trial_stitch.board if trial_stitch else trial_critical.board
            trial_deferred = (frozenset(deferred_zone_nets(trial_pre))
                              if detailed_options.defer_zone_nets else frozenset())
            trial_critical, trial_stitch, _, trial_pre = _release_escape_masks(
                trial_reserved, trial_critical, trial_stitch, None, trial_pre, unified_options=unified_options,
                on_progress=on_progress)
            trial_detailed, trial_drc, trial_duplicate = _route_ordinary_stage(
                trial_route, trial_pre, trial_global, detailed_options, None,
                drc_policy=drc_policy, routing_nets=None, on_progress=on_progress)
            return FeedbackState(trial_board, trial_global, trial_critical, trial_detailed, trial_drc,
                                 trial_deferred, (trial_stitch, trial_duplicate,
                                                  trial_inputs[-1] if inputs_path else None))

        try:
            placement_feedback = run_placement_feedback(
                FeedbackState(placement.board, placement.global_route, critical, detailed, drc,
                              deferred_zones, (plane_stitch, duplicate_stitch, None)),
                trials=feedback_budget, movement_nm=detailed_feedback_movement_nm,
                placement_options=placement_options, global_options=global_options,
                rerun=rerun_tail, score=_feedback_score, on_progress=on_progress)
            if placement_feedback.accepted_moves:
                best = placement_feedback.state
                placement = replace(placement, board=best.board, global_route=best.global_route,
                                    placement_candidate=placement.placement_candidate + "-congestion")
                critical, detailed, drc = best.critical, best.detailed, best.drc
                plane_stitch, duplicate_stitch, accepted_inputs = best.payload
                if accepted_inputs:
                    os.replace(accepted_inputs, inputs_path)
                    # The accepted trial's ball-grid fanout sidecar goes with it.
                    _replace_sidecar(accepted_inputs + ".bga.json", inputs_path + ".bga.json")
        finally:
            for leftover in trial_inputs:
                # Rejected trials' inputs are not this run's; never leave them behind.
                for stale in (leftover, leftover + ".bga.json"):
                    try:
                        os.remove(stale)
                    except FileNotFoundError:
                        pass
    if failed_nets and detailed_feedback_trials and preparation is None and unified_options is None:
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
            if getattr(unified_options, "direct", False):
                from .hard_macros import materialize_hard_macros
                trial_critical = replace(critical, board=materialize_hard_macros(trial_board), nets=())
            else:
                trial_critical = trial_access.critical if trial_access else route_critical_nets(
                    trial_board, trial_global, unified_options=unified_options,
                    plane_stitch_options=(critical_return_plane_options(trial_board, plane_stitch_options)
                                          if unified_options is not None else None))
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
                trial_global, _planned(detailed_options, trial_pre_detailed, escape_plan_options,
                                       unified_options),
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
    if getattr(unified_options, "direct", False):
        critical = _unified_profile_review(detailed, placement.global_route, drc)
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
        unified_options=unified_options,
        placement_feedback=placement_feedback,
    )


def _direct_critical(placement):
    """Direct unified mode: only source-fixed (macro) copper precedes negotiation."""
    from .hard_macros import materialize_hard_macros
    source = materialize_hard_macros(placement.board)
    return CriticalRoutingResult(CriticalRoutingStatus.WARNING, source, (), (), (),
        placement.global_route.routing_fingerprint, placement.global_route.routing_fingerprint)


def _replace_sidecar(source: str, target: str) -> None:
    """Move a saved-input sidecar with its inputs; a missing source removes a stale target."""
    try:
        os.replace(source, target)
    except FileNotFoundError:
        try:
            os.remove(target)
        except FileNotFoundError:
            pass


def _plan_array_escapes(board, escape_plan_options, on_progress):
    """Ball grids: which balls escape on the surface (min-cost flow, escape plan E2).

    Returns ``(array_plans, surface_balls)``, both None when the plan is off.
    Fanout tries a surface escape first for those balls; the rest take vias.
    """
    if escape_plan_options is None or not escape_plan_options.array_flow:
        return None, None
    from .escape_plan import plan_array_escapes
    array_plans = {item.component: item for item in plan_array_escapes(board, escape_plan_options)}
    emit(on_progress, "array_escape_plan", "finished", arrays=len(array_plans),
         surface=sum(len(item.surface) for item in array_plans.values()),
         vias=sum(len(item.vias) for item in array_plans.values()))
    return array_plans, frozenset(pad for item in array_plans.values() for pad in item.surface)


def _plan_lattice(board, escape_plan_options, surface_balls, on_progress):
    """Ball grids: one via site per ball that needs one (escape plan E7), or None.

    Planned before escapes, bypasses and critical routes can take the sites.
    Escapes and bypasses plan with the lattice copper in their frame (a
    bypass reaches its ball's via); critical routing sees it as keepouts; it
    joins the board after critical routing. Returns (tracks, vias, keepouts).
    """
    if escape_plan_options is None or not escape_plan_options.via_lattice:
        return None
    from .hard_macros import macro_source, materialize_hard_macros
    from .via_lattice import plan_via_lattices
    frame = materialize_hard_macros(macro_source(board))
    planned = plan_via_lattices(frame, surface=surface_balls,
                                band_rings=escape_plan_options.lattice_band_rings)
    emit(on_progress, "via_lattice", "finished", **planned.report())
    if not planned.vias:
        return None
    return (planned.tracks, planned.vias,
            _copper_keepouts(board, planned.tracks, planned.vias, "lattice-reserve", fixed=frame))


def _route_critical_stage(placement, *, unified_options, unified_fanout, package_access_options,
                          on_progress, on_stage, plane_stitch_options=None, lattice=None):
    """Escape and bypass reservations, then critical routing, on ``placement``.

    Shared by the main path and the congestion feedback rerun (which passes
    ``on_stage=None``). Returns ``(critical, reserved, held, held_as_copper)``.
    ``lattice`` (``_plan_lattice``) is copper escapes and bypasses plan
    around and keepouts while critical nets route.
    """
    reserved = held = None
    held_as_copper = False
    from .critical import promote_planar_nets
    board = placement.board
    # Planar ordinary nets route in the critical stage (route_critical_nets
    # promotes them after its guide checks); the planners leave them alone.
    planar = promote_planar_nets(board)[1] if unified_options is not None else ()
    if planar:
        emit(on_progress, "planar_nets", "promoted", nets=sorted(planar))
    halos = _macro_port_halos(board) if unified_options is not None else ()
    prefix = lattice[:2] if lattice else ((), ())
    if (unified_options is not None and unified_fanout is not None
            and getattr(unified_options, "escapes_first", True)):
        # Ordinary pins of crowded packages get escapes planned before the
        # critical router runs; the critical router sees them as keepouts,
        # so it cannot wall those pins in (one exact pass, no gate).
        reserved = _plan_ordinary_escapes(board, unified_fanout, on_progress, prefix=prefix, keepouts=halos,
                                          exclude=planar)
    if (unified_options is not None and board.decoupling_links
            and getattr(unified_options, "pre_route_decouplers", True)):
        # Bypasses route before critical nets (as package access did), so
        # a critical route cannot take a capacitor's short surface feed.
        held, reserved = _plan_decouplers(board, reserved, on_progress, prefix=prefix, extra_keepouts=halos,
                                          fanout_options=unified_fanout)
        if reserved is not None and held[3]:
            reserved, held, restored = _revalidate_released_escapes(board, reserved, held)
            emit(on_progress, "ordinary_escapes", "revalidated", **restored)
            if restored.get("unrestored_nets") and unified_fanout is not None:
                reserved = _resite_escapes(board, unified_fanout, reserved, held, prefix, halos,
                                           frozenset(restored["unrestored_nets"]), on_progress)
        # The witnesses were admitted in the planning context (port halos,
        # pre-release reservations); the critical router's context has only
        # the final reservations, so they are verified again there.
        held, reason = _refresh_bypass_witnesses(board, reserved, held)
        if reason is not None:
            emit(on_progress, "bypass_reservation", "witnesses_stale", reason=reason)
    def critical_inputs(escape_keepouts, previous=None):
        source = replace(board, copper_keepouts=(*board.copper_keepouts, *escape_keepouts))
        bypass_accesses, reason = (_bypass_copper_reservations(source, held[0], held[1], held[4])
                                  if held is not None else (None, None))
        if bypass_accesses is None and previous is not None and previous[1] is not None:
            from .bypass_reservations import rebind_bypass_keepout_removal
            bypass_accesses, reason = rebind_bypass_keepout_removal(previous[0], previous[1], source)
        if held is not None and bypass_accesses is None:
            # Unsupported/ambiguous ownership keeps its original conservative
            # reservation. Never drop it or infer semantic authority by net.
            source = replace(source, copper_keepouts=(*source.copper_keepouts, *held[2]))
        emit(on_progress, "bypass_reservation", "bound",
             mode="copper" if bypass_accesses is not None else "keepouts", reason=reason)
        if lattice is not None:
            # Lattice vias are reserved for their balls while critical
            # nets route; they join the board afterwards.
            source = replace(source, copper_keepouts=(*source.copper_keepouts, *lattice[2]))
        return source, bypass_accesses
    source, bypass_accesses = critical_inputs(reserved[2] if reserved else ())
    held_as_copper = bypass_accesses is not None
    return_options = (critical_return_plane_options(source, plane_stitch_options, halos)
                      if unified_options is not None else None)
    if on_stage is not None:
        on_stage("critical-inputs", {"board": source, "global_route": placement.global_route,
                                     "reserved_accesses": bypass_accesses,
                                     "plane_stitch_options": return_options})
    pair_limit = (package_access_options.initial_pair_state_limit or None
                  if package_access_options is not None else None)
    pair_limit = getattr(unified_options, "critical_pair_state_limit", None) or pair_limit
    critical = route_critical_nets(source, placement.global_route,
        reserved_accesses=bypass_accesses, pair_state_limit=pair_limit,
        plane_stitch_options=return_options,
        on_progress=critical_progress(on_progress), unified_options=unified_options)
    if reserved is not None:
        failed = {name for item in critical.nets if not item.connected for name in item.nets}
        if failed:
            # Critical nets come first: free the reservations around the
            # pads of any critical net they blocked and route again; keep
            # whichever attempt connects more critical nets.
            kept = _release_reservations(board, reserved[2], failed)
            if len(kept) < len(reserved[2]):
                reason = None
                if getattr(unified_options, "retain_escape_copper", False):
                    plan = ordinary_escape_retry_plan(reserved, kept)
                    retry_source, retry_accesses, reason = bind_ordinary_escape_copper(source, bypass_accesses, plan)
                    emit(on_progress, "ordinary_escape_copper", "bound" if reason is None else "rejected",
                         reason=reason, nets=sorted({e.net for e in (*plan[0], *plan[1])}),
                         tracks=len(plan[0]), vias=len(plan[1]))
                    kept = tuple(k for k in reserved[2] if k not in plan[2])
                else:
                    emit(on_progress, "ordinary_escapes", "released",
                         nets=sorted(failed), keepouts=len(reserved[2]) - len(kept))
                    retry_source, retry_accesses = critical_inputs(kept, (source, bypass_accesses))
                # A rejected conversion cannot authorize dropping the exits.
                # The incumbent remains untouched; no trial runs in this case.
                if reason is not None:
                    retry_source = None
                if retry_source is not None and on_stage is not None:
                    on_stage("critical-retry-inputs", {"board": retry_source,
                        "global_route": placement.global_route, "reserved_accesses": retry_accesses,
                        "plane_stitch_options": return_options})
                retry = (route_critical_nets(retry_source, placement.global_route,
                    reserved_accesses=retry_accesses, pair_state_limit=pair_limit,
                    plane_stitch_options=return_options,
                    on_progress=critical_progress(on_progress), unified_options=unified_options)
                    if retry_source is not None else None)
                if retry is not None and sum(not item.connected for item in retry.nets) < len(
                        [item for item in critical.nets if not item.connected]):
                    critical = retry
                    reserved = (reserved[0], reserved[1], kept)
                    held_as_copper = retry_accesses is not None
    return critical, reserved, held, held_as_copper


def _macro_port_halos(board):
    """Keepouts beyond macro ports whose nets may change layer, for the planners.

    A pair leaving a hard macro's port needs a via transition just beyond
    the port (the FX10 USB escapes). Escape and bypass planning run before
    critical routing and would put return vias and supply tracks there; the
    critical router then finds no legal transition. Reference layouts keep
    the channel beyond a pair's package exit free. The halo covers every
    copper layer from one clearance past the port, two via pitches outward
    and one to each side; only the planners see it, never the routers.
    """
    from .hard_macros import macro_source, materialize_hard_macros, resolved_macro_geometry
    from .physical import CopperKeepout, Point, PolygonRing, PolygonWithHoles
    frame = materialize_hard_macros(macro_source(board))
    rules = {rule.net: rule for rule in board.net_routing_rules}
    poses = {pose.reference: pose for pose in board.placements}
    gap = board.rules.minimum_clearance_nm
    halos = []
    for macro in frame.hard_macros:
        if macro.cluster not in frame.materialized_macros:
            continue
        for port in resolved_macro_geometry(frame, macro)[2]:
            rule = rules.get(port.net)
            if rule is None or rule.max_vias == 0 or not port.pads:
                continue
            centre = poses[min(port.pads).component].position
            dx, dy = port.position.x_nm - centre.x_nm, port.position.y_nm - centre.y_nm
            ox, oy = ((1 if dx > 0 else -1), 0) if abs(dx) >= abs(dy) else (0, (1 if dy > 0 else -1))
            half = (board.rules.default_via_size_nm + gap
                    + (rule.width_nm or board.rules.default_track_width_nm) + (rule.pair_gap_nm or 0))
            x0, y0 = port.position.x_nm + ox * gap, port.position.y_nm + oy * gap
            if ox:
                xs, ys = sorted((x0, x0 + ox * 2 * half)), (y0 - half, y0 + half)
            else:
                xs, ys = (x0 - half, x0 + half), sorted((y0, y0 + oy * 2 * half))
            ring = PolygonRing((Point(xs[0], ys[0]), Point(xs[1], ys[0]), Point(xs[1], ys[1]), Point(xs[0], ys[1])))
            halos.append(CopperKeepout(f"port-halo-{len(halos)}", tuple(board.stackup.copper_layers),
                                       PolygonWithHoles(ring), block_tracks=True, block_vias=True,
                                       block_zones=False))
    return tuple(halos)


def _refresh_bypass_witnesses(board, reserved, held):
    """Verify the admitted bypass witnesses in the keepout context critical routing sees.

    A witness carries the digest of the board it was admitted on, keepouts
    included. Planning admits them on a frame with the port halos and the
    pre-release escape reservations; the critical source carries the final
    reservations only, so each witness is checked again there, exactly as
    the escape revalidation does. A witness that fails keeps ``held`` as it
    is (bypass copper then binds as keepouts) and the reason is returned.
    """
    from .decoupling_paths import verify_crosslayer_decoupling
    from .hard_macros import macro_source, materialize_hard_macros
    from .routing_clearance import RoutingClearanceIndex
    frame = materialize_hard_macros(macro_source(board))
    fixed = replace(frame, tracks=(*frame.tracks, *held[0]), vias=(*frame.vias, *held[1]))
    context = replace(fixed, copper_keepouts=(*board.copper_keepouts, *(reserved[2] if reserved else ())))
    fresh = RoutingClearanceIndex(context)
    witnesses = []
    for witness in held[4]:
        checked = verify_crosslayer_decoupling(context, witness.link, return_target=witness.return_target,
                                               index=fresh)
        if checked.witness is None:
            return held, checked.reason
        witnesses.append(checked.witness)
    return (*held[:4], tuple(witnesses)), None


def _resite_escapes(board, fanout_options, reserved, held, prefix, halos, nets, on_progress):
    """Plan the exits of ``nets`` again around the bypass copper that displaced them.

    Bypass planning releases escape reservations near a blocked bypass and
    revalidation restores only the groups still legal beside the bypass
    copper. A ball whose dogbone site that copper took is otherwise left to
    the fanout stage, which finds its cell centres taken (CopperProbe K7,
    K9). The new escapes are planned with the bypass copper as prefix and the
    remaining reservations as keepouts; reservation ids stay index-stable.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    prefix_tracks = (*prefix[0], *held[0])
    prefix_vias = (*prefix[1], *held[1])
    again = _plan_ordinary_escapes(board, fanout_options, on_progress, prefix=(prefix_tracks, prefix_vias),
                                   keepouts=(*reserved[2], *halos), only_nets=nets, event="resited")
    if again is None:
        return reserved
    tracks = (*reserved[0], *again[0])
    vias = (*reserved[1], *again[1])
    frame = materialize_hard_macros(macro_source(board))
    fixed = replace(frame, tracks=(*frame.tracks, *prefix_tracks), vias=(*frame.vias, *prefix_vias))
    released = {keepout.id for keepout in held[3]}
    keepouts = tuple(keepout for keepout in _copper_keepouts(board, tracks, vias, "escape-reserve", fixed=fixed)
                     if keepout.id not in released)
    return tracks, vias, keepouts


def _complete_critical_stage(critical, reserved, held, held_as_copper, *, access, unified_options,
                             plane_stitch_options, on_progress, lattice=None, array_plans=None,
                             escape_plan_options=None, save_inputs_path=None, fanout_options=None):
    """Lattice and held bypass copper, decouplers, power islands, BGA dogbones and plane stitch.

    Returns ``(critical, plane_stitch)``. ``save_inputs_path`` (the unified
    router's saved-inputs path of this run or trial) gets the ball-grid
    fanout copper beside it, in ``PATH.bga.json``, when there is any.
    ``fanout_options`` (the stage's escape options) give the bypass links
    left pending at planning their late pass with the same launch options
    (maze launches included) beside the critical copper.
    """
    if lattice is not None:
        # The lattice copper joins the board first (bypasses were planned on it).
        critical = replace(critical, board=_join_reserved_copper(critical.board, *lattice))
    if held is not None and not held_as_copper:
        # The critical router kept clear of the reserved bypass copper, so it
        # joins the board unchanged.
        critical = replace(critical, board=_join_reserved_copper(critical.board, *held[:3]))
    if (access is None and critical.board.decoupling_links
            and not getattr(unified_options, "direct", False)
            and getattr(unified_options, "pre_route_decouplers", True)):
        from .decoupling import route_decouplers
        before = {link for link in critical.board.decoupling_links}
        emit(on_progress, "bypass_completion", "started", links=len(before))
        bypass = route_decouplers(critical.board, fanout_options=fanout_options)
        critical = replace(critical, board=bypass.board)
        pending = frozenset(link for link, _ in bypass.pending)
        reasons = {link: reason for link, reason in bypass.pending}
        if reserved is not None and pending:
            # A bypass the reservations blocked gets them released around its
            # own capacitor and target pad only, then one more attempt.
            pads = {pad for link in pending for pad in (link.capacitor, link.target, link.return_pad)}
            kept = _release_reservations(critical.board, reserved[2], frozenset(), pads=pads,
                                         margin_nm=nm_from_mm("0.5"))
            if len(kept) < len(reserved[2]):
                temporary = {keepout.id for keepout in reserved[2]} - {keepout.id for keepout in kept}
                board = critical.board
                retry = route_decouplers(replace(board, copper_keepouts=tuple(
                    k for k in board.copper_keepouts if k.id not in temporary)), only_links=pending,
                    fanout_options=fanout_options)
                if len(retry.pending) < len(pending):
                    critical = replace(critical, board=replace(retry.board,
                                                               copper_keepouts=board.copper_keepouts))
                    pending = frozenset(link for link, _ in retry.pending)
                    reasons = {link: reason for link, reason in retry.pending}
        # The planning event listed the links pending then; the difference is
        # what the late pass routed beside the critical copper.
        emit(on_progress, "bypass_completion", "finished", pending=len(pending),
             connected=sorted(_link_name(link) for link in before - pending
                              if link.capacitor in bypass.connected_pads),
             pending_links=[{"link": _link_name(link), "reason": reasons[link]} for link in sorted(
                 pending, key=_link_name)])
    if unified_options is not None and getattr(unified_options, "power_islands", None):
        from .physical import CopperLayer
        from .power_islands import auto_power_layer, plan_power_islands
        choice = unified_options.power_islands
        layer = auto_power_layer(critical.board) if choice == "auto" else CopperLayer(choice)
        if layer is not None:
            emit(on_progress, "power_islands", "started", layer=layer.value)
            if getattr(unified_options, "power_islands_ball_grid_only", False):
                from .power_islands import plan_ball_grid_islands
                partition = plan_ball_grid_islands(critical.board, layer)
            else:
                partition = plan_power_islands(critical.board, layer)
            critical = replace(critical, board=replace(
                critical.board, zones=(*critical.board.zones, *partition.zones)))
            emit(on_progress, "power_islands", "finished", **partition.report())
    if unified_options is not None and getattr(unified_options, "bga_fanout", True):
        from .bga_fanout import plan_bga_fanouts
        emit(on_progress, "bga_fanout", "started")
        bga = plan_bga_fanouts(critical.board, array_plans=array_plans)
        critical = replace(critical, board=bga.board)
        emit(on_progress, "bga_fanout", "finished", **bga.report())
        if save_inputs_path and (bga.tracks or bga.vias):
            # Beside the router inputs: the fanout copper, so a replay can
            # take it out and fan out again (replay_unified(bga_fanout=...)).
            from .state_codec import save_state
            save_state(save_inputs_path + ".bga.json",
                       {"tracks": bga.tracks, "vias": bga.vias, "array_flow": array_plans is not None,
                        "escape_plan_options": escape_plan_options},
                       note="bga_fanout output")
    plane_stitch = access.plane_stitch if access else (
        stitch_zone_pads(critical.board, plane_stitch_options)
        if plane_stitch_options else None
    )
    return critical, plane_stitch


def _release_escape_masks(reserved, critical, plane_stitch, fanout, pre_detailed, *, unified_options=None,
                          on_progress=None):
    """Drop the temporary escape-reservation keepouts before ordinary routing.

    With ``unified_options.planned_escapes_as_copper`` the promised exits
    join the ordinary stage's board as copper (each piece exact-checked, a
    piece the preparation steps made illegal is left out and reported):
    the stage's fanout then keeps them instead of finding exits again on a
    board that critical copper, bypasses and plane contacts have filled.
    """
    if reserved is None:
        return critical, plane_stitch, fanout, pre_detailed
    # Keep promised exits through every preparation step, including plane
    # contacts. Remove only these temporary masks when the shared ordinary
    # router takes ownership; a later GND via must not occupy the launch.
    temporary = {keepout.id for keepout in reserved[2]}
    def release_masks(current):
        return replace(current,copper_keepouts=tuple(
            k for k in current.copper_keepouts if k.id not in temporary))
    critical = replace(critical,board=release_masks(critical.board))
    if plane_stitch is not None:
        plane_stitch = replace(plane_stitch,board=release_masks(plane_stitch.board))
    if fanout is not None:
        fanout = replace(fanout,board=release_masks(fanout.board))
    pre_detailed = release_masks(pre_detailed)
    if getattr(unified_options, "planned_escapes_as_copper", False) and (reserved[0] or reserved[1]):
        joined = _join_reserved_copper(pre_detailed, reserved[0], reserved[1], ())
        kept_tracks = joined.tracks[len(pre_detailed.tracks):]
        kept_vias = joined.vias[len(pre_detailed.vias):]
        dropped = sorted({item.net for item in (*reserved[0], *reserved[1])}
                         - {item.net for item in (*kept_tracks, *kept_vias)}
                         | {v.net for v in reserved[1] if v not in kept_vias})
        emit(on_progress, "ordinary_escapes", "joined", tracks=len(kept_tracks), vias=len(kept_vias),
             dropped_tracks=len(reserved[0]) - len(kept_tracks), dropped_vias=len(reserved[1]) - len(kept_vias),
             dropped_nets=dropped)
        pre_detailed = joined
    return critical, plane_stitch, fanout, pre_detailed


def _route_ordinary_stage(route_ordinary, pre_detailed, global_route, detailed_options, fanout, *,
                          drc_policy, routing_nets, on_progress):
    """Ordinary routing then land closure and DRC; returns ``(detailed, drc, duplicate_stitch)``."""
    emit(on_progress, "ordinary_area", "started")
    detailed = route_ordinary(
        pre_detailed, global_route,
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
    return detailed, drc, duplicate_stitch


def _unified_profile_review(detailed, global_route, drc):
    """Report final profile copper without calling a legacy routing solver.

    Independent exact paths do not certify differential coupling/returns. Keep
    that unsupported obligation explicit and fail closed for those profiles.
    """
    from math import hypot
    from .critical import CriticalNetResult
    from .drc import ROUTING_BUDGET_WARNING_CODES, DrcSeverity, explicit_copper_connectivity
    board = detailed.board
    graph = explicit_copper_connectivity(board)
    nets = {net.name: net for net in board.nets}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    results, seen = [], set()
    for name, rule in sorted(rules.items()):
        if rule.kind is RouteKind.GENERAL or name in seen:
            continue
        group = tuple(sorted({name, rule.differential_partner})) if rule.differential_partner else (name,)
        seen.update(group)
        tracks = tuple(t for t in board.tracks if t.net in group)
        vias = tuple(v for v in board.vias if v.net in group)
        lengths = tuple(sum(round(hypot(t.end.x_nm-t.start.x_nm, t.end.y_nm-t.start.y_nm))
                            for t in tracks if t.net == net) for net in group)
        connected = all(net in nets and graph.net_connected(nets[net]) for net in group)
        diagnostics = [f.message for f in drc.findings
                       if f.severity is DrcSeverity.ERROR and set(f.nets).intersection(group)]
        warnings = tuple(dict.fromkeys(f.message for f in drc.findings
                         if f.code in ROUTING_BUDGET_WARNING_CODES and set(f.nets).intersection(group)))
        if not connected:
            diagnostics.append("unified routing left profile terminals disconnected")
        if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}:
            diagnostics.append("unified differential coupling and return-path verification is not implemented")
        results.append(CriticalNetResult(group, connected and not diagnostics, len(tracks), len(vias),
            lengths, max(lengths)-min(lengths) if lengths else 0,
            tuple(dict.fromkeys(diagnostics)), strategy="unified_final_review", warnings=warnings))
    status = (CriticalRoutingStatus.FAILED if any(not item.connected for item in results)
              else CriticalRoutingStatus.WARNING if any(item.warnings for item in results)
              else CriticalRoutingStatus.SUCCESS)
    return CriticalRoutingResult(status, board, tuple(results), (), (),
        global_route.routing_fingerprint, detailed.routing_fingerprint)


def critical_return_plane_options(board, options, keepouts=()):
    """Prepare declared return contacts before a critical search needs them.

    Respect the caller's early-contact opt-out and pad selection. The critical
    router owns this freshly generated prefix separately from ordinary escapes.
    Zone outlines remain intent, not evidence of actual filled connectivity.
    ``keepouts`` (the macro port halos) bind the stitch only: a return via
    for one pair must not take the launch corridor of another pair's ports
    (CopperProbe TX1_IC, walled in by the stitch vias of nearby capacitors).
    """
    if options is None:
        return None
    returns = {r.return_via_net for r in board.net_routing_rules
               if r.kind is not RouteKind.GENERAL and r.require_return_vias}
    returns &= {zone.net for zone in board.zones}
    pads = frozenset(p for net in board.nets if net.name in returns for p in net.pads)
    if options.only_pads is not None:
        pads &= options.only_pads
    return replace(options, only_pads=pads, keepouts=(*options.keepouts, *keepouts)) if pads else None


def ordinary_escape_retry_plan(planned, kept):
    """Whole owned escape groups affected by a proposed temporary-mask release."""
    tracks, vias, masks = planned
    owners = {**{f"escape-reserve-track-{i}": t.net for i,t in enumerate(tracks)},
              **{f"escape-reserve-via-{i}": v.net for i,v in enumerate(vias)}}
    if not set(kept) <= set(masks) or any(k.id not in owners for k in masks):
        raise ValueError("stale ordinary escape retry ownership")
    affected = {owners[k.id] for k in masks if k not in kept}
    return (tuple(t for t in tracks if t.net in affected),
            tuple(v for v in vias if v.net in affected),
            tuple(k for k in masks if owners[k.id] in affected))


def bind_ordinary_escape_copper(source, bypass, planned):
    """Atomically replace temporary exit masks by the complete checked plan.

    No individual edge is filtered to obtain admission. Rejected plans keep the
    incumbent source, masks and bypass witnesses unchanged.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    from .bypass_reservations import rebind_bypass_keepout_removal, verified_bypass_reservations
    from .drc import DrcSeverity
    from collections import Counter
    tracks, vias, masks = planned
    temporary = {mask.id for mask in masks}
    if any(mask not in source.copper_keepouts for mask in masks):
        return source, bypass, "stale ordinary escape masks"
    clean = replace(source, copper_keepouts=tuple(k for k in source.copper_keepouts if k.id not in temporary))
    frame = materialize_hard_macros(macro_source(clean))
    if bypass is not None:
        rebound, reason = rebind_bypass_keepout_removal(source, bypass, clean)
        if reason is not None:
            return source, bypass, reason
    else:
        rebound = FanoutResult(frame, {}, (), 0, 0)
    if (len(set(tracks)) != len(tracks) or len(set(vias)) != len(vias)
            or Counter(tracks) & Counter(rebound.board.tracks)
            or Counter(vias) & Counter(rebound.board.vias)):
        return source, bypass, "ordinary exit plan has duplicate or immutable ownership"
    rules = {r.net: r for r in frame.net_routing_rules}
    supply = {z.net for z in frame.zones}
    supply |= {l.net for l in frame.decoupling_links} | {l.return_net for l in frame.decoupling_links}
    known = {n.name for n in frame.nets}
    if any(e.net not in known or e.net in supply or
           (e.net in rules and rules[e.net].kind is not RouteKind.GENERAL) for e in (*tracks, *vias)):
        return source, bypass, "ordinary exit plan contains unknown or protected copper"
    candidate = replace(rebound,
        board=replace(rebound.board, tracks=(*rebound.board.tracks, *tracks), vias=(*rebound.board.vias, *vias)),
        created_tracks=(*rebound.created_tracks, *tracks), created_vias=(*rebound.created_vias, *vias),
        added_track_count=rebound.added_track_count + len(tracks),
        added_via_count=rebound.added_via_count + len(vias))
    _, _, reason = verified_bypass_reservations(frame, candidate)
    if reason is not None:
        return source, bypass, reason
    def errors(board):
        return {f.fingerprint for f in run_physical_drc(board).findings
                if f.severity is DrcSeverity.ERROR and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}}
    if errors(candidate.board) - errors(frame):
        return source, bypass, "ordinary escape copper introduces physical violations"
    return clean, candidate, None


def _plan_ordinary_escapes(board, fanout_options, on_progress, *, prefix=((), ()), keepouts=(),
                           only_nets=None, exclude=(), event="finished"):
    """Escapes for crowded ordinary pins, and keepouts that reserve them.

    Returns (tracks, vias, keepouts) on the macro-materialized frame, or None.
    ``prefix`` (tracks, vias) is planned copper the frame already holds (a
    via lattice, bypass copper); it is not part of the result. ``keepouts``
    bind the planning only. ``only_nets`` limits the nets planned.
    """
    rules = {rule.net: rule for rule in board.net_routing_rules}
    # Signal nets only: supply and return nets are decoupled or planes, and
    # leaving them out also keeps the escape generator from routing bypasses.
    supply = {zone.net for zone in board.zones}
    supply |= {link.net for link in board.decoupling_links} | {link.return_net for link in board.decoupling_links}
    ordinary = frozenset(net.name for net in board.nets
                         if len(net.pads) >= 2 and net.name not in supply
                         and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL))
    if only_nets is not None:
        ordinary &= frozenset(only_nets)
    ordinary -= frozenset(exclude)
    if not ordinary:
        return None
    emit(on_progress, "ordinary_escapes", "started", nets=len(ordinary))
    try:
        tracks, vias, reserved, pending = _route_ordinary_escapes(board, fanout_options, ordinary,
                                                                  prefix=prefix, keepouts=keepouts)
    except ValueError as error:
        emit(on_progress, "ordinary_escapes", event, skipped=str(error))
        return None
    emit(on_progress, "ordinary_escapes", event, vias=len(vias), tracks=len(tracks),
         pending=len(pending), nets=sorted({item.net for item in (*tracks, *vias)}))
    return tracks, vias, reserved


def _route_ordinary_escapes(board, fanout_options, nets, *, prefix=((), ()), keepouts=()):
    """Escapes of ``nets`` on the macro frame: (tracks, vias, keepouts, pending pads).

    The frame holds ``prefix`` copper and binds ``keepouts``; neither is part
    of the result. Raises ValueError when the fanout refuses the frame.
    """
    from collections import Counter
    from .fanout import route_fanout
    from .hard_macros import macro_source, materialize_hard_macros
    frame = materialize_hard_macros(macro_source(board))
    frame = replace(frame, tracks=(*frame.tracks, *prefix[0]), vias=(*frame.vias, *prefix[1]),
                    copper_keepouts=(*frame.copper_keepouts, *keepouts))
    escapes = route_fanout(frame, fanout_options, only_nets=frozenset(nets))
    tracks = tuple((Counter(escapes.board.tracks) - Counter(frame.tracks)).elements())
    vias = tuple((Counter(escapes.board.vias) - Counter(frame.vias)).elements())
    reserved = _copper_keepouts(board, tracks, vias, "escape-reserve", fixed=frame)
    return tracks, vias, tuple(reserved), tuple(escapes.pending_pads)


def _join_reserved_copper(board, tracks, vias, keepouts):
    """Drop a stage's reservation keepouts and add the copper they held.

    Each piece is checked exactly: a reservation left out next to fixed macro
    copper did not bind the critical router there.
    """
    from .routing_clearance import RoutingClearanceIndex
    temporary = {keepout.id for keepout in keepouts}
    board = replace(board, copper_keepouts=tuple(k for k in board.copper_keepouts if k.id not in temporary))
    index = RoutingClearanceIndex(board)
    kept_tracks, kept_vias = [], []
    for via in vias:
        if index.can_via(via.net, via.position, via.size_nm, via.from_layer, via.to_layer,
                         drill_nm=via.drill_nm):
            index.add_via(via)
            kept_vias.append(via)
    for track in tracks:
        if index.can_track(track.net, track.start, track.end, track.width_nm, track.layer):
            index.add_track(track)
            kept_tracks.append(track)
    return replace(board, tracks=(*board.tracks, *kept_tracks), vias=(*board.vias, *kept_vias))


def _copper_keepouts(board, tracks, vias, prefix, fixed=None):
    """Keepouts covering planned copper plus clearance, for one routing stage.

    A keepout that would overlap ``fixed`` copper (materialized hard macros,
    which the planned copper may join) is left out: keepouts bind every net,
    and fixed copper must stay legal.
    """
    import math
    from .drc import _segment_distance
    from .physical import CopperKeepout, Point, PolygonRing, PolygonWithHoles
    gap = board.rules.minimum_clearance_nm
    layers_all = board.stackup.copper_layers
    solid = []  # (a, b, half_width, layers) of fixed copper
    if fixed is not None:
        solid += [(t.start, t.end, t.width_nm / 2, {t.layer}) for t in fixed.tracks]
        solid += [(v.position, v.position, v.size_nm / 2,
                   set(layers_all[layers_all.index(v.from_layer):layers_all.index(v.to_layer) + 1]))
                  for v in fixed.vias]

    def touches(a, b, radius, layers):
        for c, d, half, other in solid:
            # The keepout is a rectangle with square ends: its corners reach
            # radius * sqrt(2) from the copper centre line.
            if layers & other and _segment_distance(a, b, c, d) < radius * 1.42 + half:
                return True
        return False

    def capsule(a, b, radius):
        dx, dy = b.x_nm - a.x_nm, b.y_nm - a.y_nm
        length = math.hypot(dx, dy)
        ux, uy = (dx / length, dy / length) if length else (1.0, 0.0)
        nx, ny = -uy, ux
        corners = []
        for (px, py), sx in (((a.x_nm, a.y_nm), -1), ((b.x_nm, b.y_nm), 1)):
            for sn in ((-1, 1) if sx < 0 else (1, -1)):
                corners.append(Point(int(round(px + sx * radius * ux + sn * radius * nx)),
                                     int(round(py + sx * radius * uy + sn * radius * ny))))
        return PolygonWithHoles(PolygonRing(tuple(corners)))

    keepouts = []
    for index, track in enumerate(tracks):
        if touches(track.start, track.end, track.width_nm // 2 + gap, {track.layer}):
            continue
        keepouts.append(CopperKeepout(f"{prefix}-track-{index}", (track.layer,),
                                      capsule(track.start, track.end, track.width_nm // 2 + gap),
                                      block_tracks=True, block_vias=True, block_zones=False))
    layers = board.stackup.copper_layers
    for index, via in enumerate(vias):
        span = layers[layers.index(via.from_layer):layers.index(via.to_layer) + 1]
        if touches(via.position, via.position, via.size_nm // 2 + gap, set(span)):
            continue
        keepouts.append(CopperKeepout(f"{prefix}-via-{index}", tuple(span),
                                      capsule(via.position, via.position, via.size_nm // 2 + gap),
                                      block_tracks=True, block_vias=True, block_zones=False))
    return tuple(keepouts)


def _bypass_copper_reservations(source, tracks, vias, witnesses=()):
    """Fresh admission of the exact planned bypass contribution, without routing.

    Keep real net/shape/hole identity rather than inflating it into all-net
    rectangular keepouts. The ordinary public critical-prefix admission still
    verifies every source field, occurrence, complete association and hard DRC.
    No generated piece is filtered out to manufacture a valid ownership claim.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    from .bypass_reservations import verified_bypass_reservations, restore_bypass_reservation_edges
    frame = materialize_hard_macros(macro_source(source))
    reserved = FanoutResult(replace(frame, tracks=(*frame.tracks, *tracks), vias=(*frame.vias, *vias)),
        {}, (), len(tracks), len(vias), created_tracks=tracks, created_vias=vias,
        decoupling_tracks=tracks, decoupling_vias=vias, decoupling_witnesses=witnesses)
    # Re-materializing immutable macro/array copper preserves values, but its
    # source objects are fresh. Restore only unique equal original occurrences,
    # then run the complete current ownership/context/geometry gate as usual.
    try:
        reserved = restore_bypass_reservation_edges(reserved)
    except ValueError as exc:
        return None, str(exc)
    _, _, reason = verified_bypass_reservations(frame, reserved)
    return (reserved, None) if reason is None else (None, reason)


def _plan_decouplers(board, reserved, on_progress, *, prefix=((), ()), extra_keepouts=(),
                     fanout_options=None):
    """Bypass copper routed before critical nets, held as keepouts meanwhile.

    The critical router needs a board without ordinary copper, so bypasses are
    planned on the macro-materialized frame (respecting the escape
    reservations ``reserved``), reserved as keepouts while critical nets
    route, and added back afterwards.

    A link the reservations block is tried again, one link at a time, with
    the reservations within 1.5 mm of its pads lifted. Its copper is accepted
    as a swap: a reservation the new copper leaves legal stays bound (the
    lifted box is the candidate set, not the release), and every ball whose
    reservation the copper does take is planned again beside it before the
    link is accepted (``fanout_options``). A link whose copper would leave a
    ball without a site stays pending, naming the ball. Without
    ``fanout_options`` the reservations the copper takes are released for the
    caller to restore or re-site.

    Returns ``(held, reserved)``: ``held`` is ``(tracks, vias, keepouts,
    released, witnesses)`` and ``reserved`` the reservations after the swaps.
    """
    from collections import Counter
    from .decoupling import route_decouplers
    from .hard_macros import macro_source, materialize_hard_macros
    emit(on_progress, "bypass_reservation", "started", links=len(board.decoupling_links))
    tracks0, vias0, keepouts = reserved if reserved is not None else ((), (), ())
    base = materialize_hard_macros(macro_source(board))
    base = replace(base, tracks=(*base.tracks, *prefix[0]), vias=(*base.vias, *prefix[1]))
    # ``extra_keepouts`` (macro port halos) bind every attempt and are never
    # released for a pending link, unlike the escape reservations.
    frame = replace(base, copper_keepouts=(*base.copper_keepouts, *extra_keepouts, *keepouts))
    # The links plan with the stage's launch options (maze launches included).
    result = route_decouplers(frame, fanout_options=fanout_options)
    current = result.board
    pending = list(result.pending)
    witnesses = list(result.admitted_path_witnesses)
    res_tracks, res_vias, res_keepouts = list(tracks0), list(vias0), list(keepouts)
    released, swapped, rejected = [], [], []
    owners = {}

    def rebuild(fixed_tracks, fixed_vias):
        """Reservation keepouts of the current escape copper beside all bypass copper."""
        fixed = replace(base, tracks=(*base.tracks, *fixed_tracks), vias=(*base.vias, *fixed_vias))
        fresh = _copper_keepouts(board, res_tracks, res_vias, "escape-reserve", fixed=fixed)
        owners.clear()
        owners.update({f"escape-reserve-track-{i}": t.net for i, t in enumerate(res_tracks)})
        owners.update({f"escape-reserve-via-{i}": v.net for i, v in enumerate(res_vias)})
        return list(fresh)

    if keepouts:
        rebuild((), ())
    for link, reason in list(pending):
        if not res_keepouts:
            break
        if reason == "native_plane_return_pending":
            continue
        pads = (link.capacitor, link.target, link.return_pad)
        kept = _release_reservations(board, res_keepouts, frozenset(), nm_from_mm("1.5"), pads=pads)
        candidates = {k.id for k in res_keepouts} - {k.id for k in kept}
        if not candidates:
            continue
        retry_frame = replace(current, copper_keepouts=tuple(k for k in current.copper_keepouts
                                                              if k.id not in candidates))
        retry = route_decouplers(retry_frame, only_links=frozenset({link}), fanout_options=fanout_options)
        if any(item is link for item, _ in retry.pending):
            continue
        bypass_tracks = tuple((Counter(retry.board.tracks) - Counter(base.tracks)).elements())
        bypass_vias = tuple((Counter(retry.board.vias) - Counter(base.vias)).elements())
        # Exact ownership: the lifted reservations the new copper leaves legal
        # stay; only the groups it makes illegal are taken.
        candidate_nets = {owners[k] for k in candidates if k in owners}
        legal = _legal_reservation_nets(board, base, bypass_tracks, bypass_vias, res_tracks, res_vias,
                                        candidate_nets)
        taken = sorted(candidate_nets - legal)
        taken_pads = sorted({f"{p.component}.{p.pad}" for net in taken
                             for p in _reservation_pads(board, net, res_tracks)})
        if taken and fanout_options is not None:
            # The swap: every ball the copper takes is planned again beside it.
            remaining = [k for k in res_keepouts if owners.get(k.id) not in taken]
            try:
                again = _route_ordinary_escapes(board, fanout_options, taken,
                                                prefix=(bypass_tracks, bypass_vias),
                                                keepouts=(*remaining, *extra_keepouts))
            except ValueError as error:
                again = None, None, None, (str(error),)
            if again[3]:
                rejected.append({"link": _link_name(link), "balls": taken_pads,
                                 "pending": [f"{p.component}.{p.pad}" if hasattr(p, "component") else str(p)
                                             for p in again[3]]})
                pending = [(item, f"{why}; swap rejected: {', '.join(taken_pads)} would lose their site")
                           if item is link else (item, why) for item, why in pending]
                continue
            res_tracks = [t for t in res_tracks if t.net not in taken] + list(again[0])
            res_vias = [v for v in res_vias if v.net not in taken] + list(again[1])
            swapped.append({"link": _link_name(link), "balls": taken_pads})
        elif taken:
            released.extend(k for k in res_keepouts if owners.get(k.id) in taken)
            res_tracks = [t for t in res_tracks if t.net not in taken]
            res_vias = [v for v in res_vias if v.net not in taken]
        res_keepouts = rebuild(bypass_tracks, bypass_vias)
        current = replace(retry.board, copper_keepouts=(*base.copper_keepouts, *extra_keepouts, *res_keepouts))
        pending = [item for item in pending if item[0] is not link] + [
            item for item in retry.pending if item[0] is not link]
        witnesses.extend(retry.admitted_path_witnesses)
    tracks = list((Counter(current.tracks) - Counter(base.tracks)).elements())
    vias = list((Counter(current.vias) - Counter(base.vias)).elements())
    emit(on_progress, "bypass_reservation", "finished", tracks=len(tracks), vias=len(vias),
         pending=len(pending), released=len(released), swapped=swapped, rejected=rejected,
         pending_links=[{"link": _link_name(link), "reason": reason} for link, reason in pending])
    held = (tuple(tracks), tuple(vias), _copper_keepouts(board, tracks, vias, "bypass-reserve", fixed=base),
            tuple(released), tuple(witnesses))
    reserved = (tuple(res_tracks), tuple(res_vias), tuple(res_keepouts)) if reserved is not None else None
    return held, reserved


def _link_name(link):
    return f"{link.capacitor.component}.{link.capacitor.pad}->{link.target.component}.{link.target.pad}"


def _reservation_pads(board, net, tracks):
    """The pads of ``net`` that the reserved escape ``tracks`` leave from."""
    from .placement import transformed_pad_position
    poses = {pose.reference: pose for pose in board.placements}
    ends = {t.start for t in tracks if t.net == net} | {t.end for t in tracks if t.net == net}
    return [pad for item in board.nets if item.name == net for pad in item.pads
            if transformed_pad_position(board, poses[pad.component], pad.pad) in ends]


def _legal_reservation_nets(board, base, bypass_tracks, bypass_vias, tracks, vias, nets):
    """The ``nets`` whose complete escape reservation is legal beside the bypass copper.

    The group is judged whole (a legal via with a blocked stem is not legal),
    and the bypass copper must not cost it a proxy keepout either (a keepout
    overlapping fixed copper is left out), as the restoration after a release
    requires.
    """
    from .return_paths import existing_returns_permit
    from .routing_clearance import RoutingClearanceIndex
    fixed = replace(base, tracks=(*base.tracks, *bypass_tracks), vias=(*base.vias, *bypass_vias))
    index = RoutingClearanceIndex(fixed)
    rules = {r.net: r for r in board.net_routing_rules}
    legal = set()
    for net in nets:
        own_tracks = [t for t in tracks if t.net == net]
        own_vias = [v for v in vias if v.net == net]
        if not all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in own_tracks):
            continue
        if not all(index.can_via(v.net, v.position, v.size_nm, v.from_layer, v.to_layer, drill_nm=v.drill_nm)
                   and existing_returns_permit(fixed, rules.get(v.net), v) for v in own_vias):
            continue
        if (len(_copper_keepouts(board, own_tracks, own_vias, "check", fixed=fixed))
                < len(_copper_keepouts(board, own_tracks, own_vias, "check", fixed=base))):
            continue
        legal.add(net)
    return legal



def _revalidate_released_escapes(board, reserved, held):
    """Restore collateral reservations only as complete, freshly legal net groups.

    Bypass planning releases proximity boxes, not proven conflicting exits. A
    surviving complete launch must still constrain subsequent critical routing.
    No copper is changed. A new temporary keepout context gets a fresh bypass
    ledger and ordinary strict prefix admission; any failed check rolls back
    the entire restoration, without editing the original witnesses.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    from .routing_clearance import RoutingClearanceIndex
    from .return_paths import existing_returns_permit
    from .decoupling_paths import verify_crosslayer_decoupling
    frame = materialize_hard_macros(macro_source(board))
    fixed = replace(frame, tracks=(*frame.tracks, *held[0]), vias=(*frame.vias, *held[1]))
    index = RoutingClearanceIndex(fixed)
    tracks, vias, keepouts = reserved
    released = {k.id for k in held[3]}
    baseline = (tracks, vias, tuple(k for k in keepouts if k.id not in released))
    owners = {**{f"escape-reserve-track-{i}": t.net for i,t in enumerate(tracks)},
              **{f"escape-reserve-via-{i}": v.net for i,v in enumerate(vias)}}
    affected = {owners[k.id] for k in keepouts if k.id in released and k.id in owners}
    # Preserve the complete original group, not a legal via with a blocked stem.
    rules = {r.net:r for r in board.net_routing_rules}
    legal = {net for net in affected
             if all(index.can_track(t.net,t.start,t.end,t.width_nm,t.layer)
                    for t in tracks if t.net == net)
             and all(index.can_via(v.net,v.position,v.size_nm,v.from_layer,v.to_layer,drill_nm=v.drill_nm)
                     and existing_returns_permit(fixed,rules.get(v.net),v)
                     for v in vias if v.net == net)}
    # Proxy keepouts must themselves avoid the real immutable/bypass prefix.
    safe_ids = {k.id for k in _copper_keepouts(board, tracks, vias, "escape-reserve", fixed=fixed)}
    legal = {net for net in legal if all(k.id in safe_ids for k in keepouts
                                        if k.id in released and owners.get(k.id) == net)}
    kept = tuple(k for k in keepouts if k.id not in released or owners.get(k.id) in legal)
    count = len(kept) - len(baseline[2])
    report = {"restored_keepouts":0, "restored_nets":[],
              "unrestored_nets":sorted(affected), "outcome":"no_complete_legal_group"}
    if not count:
        return baseline, held, report
    context = replace(fixed,copper_keepouts=(*board.copper_keepouts,*kept))
    fresh = RoutingClearanceIndex(context)
    witnesses=[]
    for witness in held[4]:
        checked = verify_crosslayer_decoupling(context,witness.link,
                    return_target=witness.return_target,index=fresh)
        if checked.witness is None:
            return baseline, held, {**report,"outcome":"bypass_revalidation_failed","reason":checked.reason}
        witnesses.append(checked.witness)
    source = replace(board,copper_keepouts=context.copper_keepouts)
    _, reason = _bypass_copper_reservations(source,held[0],held[1],tuple(witnesses))
    if reason is not None:
        return baseline, held, {**report,"outcome":"prefix_admission_failed","reason":reason}
    remaining = tuple(k for k in held[3] if k.id not in {x.id for x in kept})
    updated = (*held[:3],remaining,tuple(witnesses))
    return (tracks,vias,kept), updated, {**report,"restored_keepouts":count,
        "restored_nets":sorted(legal),"unrestored_nets":sorted(affected-legal),"outcome":"restored"}


def _release_reservations(board, keepouts, nets, margin_nm=nm_from_mm("2"), *, pads=()):
    """Escape reservations outside the pad boxes of ``nets`` and around ``pads`` (+margin)."""
    from .placement import transformed_local_point
    poses = {pose.reference: pose for pose in board.placements}

    def where(pad):
        pose = poses[pad.component]
        land = next(l for l in board.footprints[pose.footprint].pads if l.number == pad.pad)
        return transformed_local_point(pose, land.position)

    boxes = []
    groups = [[where(pad) for pad in net.pads] for net in board.nets if net.name in nets]
    groups += [[where(pad)] for pad in pads]
    for points in groups:
        if points:
            boxes.append((min(p.x_nm for p in points) - margin_nm, min(p.y_nm for p in points) - margin_nm,
                          max(p.x_nm for p in points) + margin_nm, max(p.y_nm for p in points) + margin_nm))

    def inside(keepout):
        vertices = keepout.outline.outer.vertices
        cx = sum(v.x_nm for v in vertices) / len(vertices)
        cy = sum(v.y_nm for v in vertices) / len(vertices)
        return any(x0 <= cx <= x1 and y0 <= cy <= y1 for x0, y0, x1, y1 in boxes)

    return tuple(keepout for keepout in keepouts if not inside(keepout))


def _planned(detailed_options: DetailedRouterOptions, board: PhysicalBoard, escape_plan_options,
             unified_options) -> DetailedRouterOptions:
    """``detailed_options`` with a fresh two-end escape plan of a moved ``board``, when one is asked for."""
    if escape_plan_options is None or not escape_plan_options.two_end_order or unified_options is not None:
        return detailed_options
    from .escape_plan import plan_two_end_escapes
    plan = plan_two_end_escapes(board, escape_plan_options)
    return replace(detailed_options, escape_net_order=plan.net_order, escape_layer_biases=plan.layer_biases)


def _route_unified_compat(unified_options, board, global_route, _detailed_options, *,
                          escape_options=None, **kwargs):
    from .unified_router import route_unified
    seeder = None
    if escape_options is not None and not getattr(unified_options, "direct", False):
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


def _feedback_score(critical, detailed: DetailedRoutingResult, drc: PhysicalDrcReport) -> tuple[int, ...]:
    """Acceptance tuple of the unified placement feedback loop; lower is better.

    ``(hard DRC errors, open signal nets (DRC-OPEN-NET, deferred zone nets
    excluded), critical groups not connected or with diagnostics, unrouted
    ordinary nets, conflict overflow, total length, vias)``; see
    ``congestion_feedback.SCORE_ORDER``. The legacy loop keeps ``_detailed_score``.
    """
    hard = sum(item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
               and item.severity.value == "error" for item in drc.findings)
    from .zone_geometry import deferred_zone_nets
    zone_nets = deferred_zone_nets(detailed.board)
    opens = {net for item in drc.findings if item.code == "DRC-OPEN-NET"
             for net in item.nets if net not in zone_nets}
    critical_failures = sum(not item.connected or bool(item.diagnostics)
                            for item in getattr(critical, "nets", ()))
    return (hard, len(opens), critical_failures, detailed.metrics.unrouted_net_count,
            detailed.metrics.total_conflict_overflow, detailed.metrics.total_length_nm,
            detailed.metrics.via_count)
