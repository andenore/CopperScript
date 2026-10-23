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

    @property
    def board(self) -> PhysicalBoard:
        return self.detailed.board


def _stopped_after_placement(placement: PlacementRoutingResult, drc_policy) -> tuple:
    """Zero-pass critical/detailed results for a run stopped after placement."""

    from .critical import _fingerprint as critical_fingerprint
    from .detailed import DetailedNetResult, DetailedRoutingMetrics, _fingerprint as detailed_fingerprint
    board = replace(placement.board, metadata={**placement.board.metadata, "stopped_after": "placement",
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
    if stop_after is not None and stop_after not in STOP_STAGES:
        raise ValueError(f"stop_after must be one of {', '.join(STOP_STAGES)}")
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
                                     stopped_after="placement")
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
    reserved = held = None
    stopped = None
    if unified_options is not None and getattr(unified_options, "direct", False):
        from .hard_macros import materialize_hard_macros
        source = materialize_hard_macros(placement.board)
        critical = CriticalRoutingResult(CriticalRoutingStatus.WARNING, source, (), (), (),
            placement.global_route.routing_fingerprint, placement.global_route.routing_fingerprint)
        plane_stitch_options = None
        emit(on_progress, "unified_direct", "started",
             reason="all mutable nets negotiate together; only source-fixed copper is retained")
    elif fanout_options is not None:
        access_options = package_access_options or PackageAccessOptions()
        if preparation is None:
            access = preflight_package_access(placement.board, placement.global_route,
                                             fanout_options, plane_stitch_options,
                                             options=access_options, on_progress=on_progress)
        if stop_after == "package-preflight":
            stopped = "package-preflight"
        # With fanout enabled, critical failures belong to the same escape-first
        # placement transaction, not a second critical-first controller.
        access_options = replace(access_options,
            maximum_trials=access_options.maximum_trials + critical_feedback_trials)
        if stopped is None:
            if preparation is None:
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
                power_area_status=(preparation.status if preparation and not preparation.accepted else
                                   "package_access_incomplete") if power_area_options is not None else "disabled",
                power_area_options=power_area_options, power_preparation=preparation,
                power_native_verifier=power_native_verifier, stopped_after=stopped)
        if not access.ready:
            emit(on_progress, "package_access", "incomplete", pending_pads=len(access.pending_pads))
        critical = access.critical
    else:
        if (unified_options is not None and unified_fanout is not None
                and getattr(unified_options, "escapes_first", True)):
            # Ordinary pins of crowded packages get escapes planned before the
            # critical router runs; the critical router sees them as keepouts,
            # so it cannot wall those pins in (one exact pass, no gate).
            reserved = _plan_ordinary_escapes(placement.board, unified_fanout, on_progress)
        if (unified_options is not None and placement.board.decoupling_links
                and getattr(unified_options, "pre_route_decouplers", True)):
            # Bypasses route before critical nets (as package access did), so
            # a critical route cannot take a capacitor's short surface feed.
            held = _plan_decouplers(placement.board, reserved[2] if reserved else (), on_progress)
            if reserved is not None and held[3]:
                dropped = {keepout.id for keepout in held[3]}
                reserved = (reserved[0], reserved[1], tuple(k for k in reserved[2] if k.id not in dropped))
        source = placement.board
        extra = (*(reserved[2] if reserved else ()), *(held[2] if held else ()))
        if extra:
            source = replace(source, copper_keepouts=(*source.copper_keepouts, *extra))
        critical = route_critical_nets(source, placement.global_route,
                                       on_progress=critical_progress(on_progress))
        if reserved is not None:
            failed = {name for item in critical.nets if not item.connected for name in item.nets}
            if failed:
                # Critical nets come first: free the reservations around the
                # pads of any critical net they blocked and route again; keep
                # whichever attempt connects more critical nets.
                kept = _release_reservations(placement.board, reserved[2], failed)
                if len(kept) < len(reserved[2]):
                    emit(on_progress, "ordinary_escapes", "released",
                         nets=sorted(failed), keepouts=len(reserved[2]) - len(kept))
                    retry = route_critical_nets(
                        replace(placement.board, copper_keepouts=(*placement.board.copper_keepouts, *kept,
                                                                  *(held[2] if held else ()))),
                        placement.global_route, on_progress=critical_progress(on_progress))
                    if sum(not item.connected for item in retry.nets) < len(
                            [item for item in critical.nets if not item.connected]):
                        critical = retry
                        reserved = (reserved[0], reserved[1], kept)
    critical_feedback = None
    if critical_feedback_trials and access is None and not getattr(unified_options, "direct", False):
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
    if held is not None and critical_feedback is not None and critical_feedback.accepted_moves:
        held = None  # placement moved: the planned bypass copper no longer applies
        critical = replace(critical, board=replace(critical.board, copper_keepouts=tuple(
            k for k in critical.board.copper_keepouts if not k.id.startswith("bypass-reserve"))))
    if held is not None:
        # The critical router kept clear of the reserved bypass copper, so it
        # joins the board unchanged.
        board = critical.board
        temporary = {keepout.id for keepout in held[2]}
        board = replace(board, copper_keepouts=tuple(k for k in board.copper_keepouts if k.id not in temporary))
        # Each piece is checked exactly: a reservation left out next to fixed
        # macro copper did not bind the critical router there.
        from .routing_clearance import RoutingClearanceIndex
        index = RoutingClearanceIndex(board)
        tracks, vias = [], []
        for via in held[1]:
            if index.can_via(via.net, via.position, via.size_nm, via.from_layer, via.to_layer,
                             drill_nm=via.drill_nm):
                index.add_via(via)
                vias.append(via)
        for track in held[0]:
            if index.can_track(track.net, track.start, track.end, track.width_nm, track.layer):
                index.add_track(track)
                tracks.append(track)
        critical = replace(critical, board=replace(
            board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias)))
    if (access is None and critical.board.decoupling_links
            and not getattr(unified_options, "direct", False)
            and getattr(unified_options, "pre_route_decouplers", True)):
        from .decoupling import route_decouplers
        bypass = route_decouplers(critical.board)
        critical = replace(critical, board=bypass.board)
        pending = frozenset(link for link, _ in bypass.pending)
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
                    k for k in board.copper_keepouts if k.id not in temporary)), only_links=pending)
                if len(retry.pending) < len(pending):
                    critical = replace(critical, board=replace(retry.board,
                                                               copper_keepouts=board.copper_keepouts))
    if reserved is not None:
        # The reservations also held while decouplers routed. The escapes
        # themselves are not kept: the unified router finds its own exact
        # access, and a fixed escape of one pin can block a neighbour that the
        # escape generator left pending.
        temporary = {keepout.id for keepout in reserved[2]}
        board = critical.board
        critical = replace(critical, board=replace(
            board, copper_keepouts=tuple(k for k in board.copper_keepouts if k.id not in temporary)))
    if unified_options is not None and getattr(unified_options, "power_islands", None):
        from .physical import CopperLayer
        from .power_islands import auto_power_layer, plan_power_islands
        choice = unified_options.power_islands
        layer = auto_power_layer(critical.board) if choice == "auto" else CopperLayer(choice)
        if layer is not None:
            emit(on_progress, "power_islands", "started", layer=layer.value)
            partition = plan_power_islands(critical.board, layer)
            critical = replace(critical, board=replace(
                critical.board, zones=(*critical.board.zones, *partition.zones)))
            emit(on_progress, "power_islands", "finished", **partition.report())
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
            if getattr(unified_options, "direct", False):
                from .hard_macros import materialize_hard_macros
                trial_critical = replace(critical, board=materialize_hard_macros(trial_board), nets=())
            else:
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
    )


def _unified_profile_review(detailed, global_route, drc):
    """Report final profile copper without calling a legacy routing solver.

    Independent exact paths do not certify differential coupling/returns. Keep
    that unsupported obligation explicit and fail closed for those profiles.
    """
    from math import hypot
    from .critical import CriticalNetResult
    from .drc import DrcSeverity, explicit_copper_connectivity
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
        if not connected:
            diagnostics.append("unified routing left profile terminals disconnected")
        if rule.kind in {RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS}:
            diagnostics.append("unified differential coupling and return-path verification is not implemented")
        results.append(CriticalNetResult(group, connected and not diagnostics, len(tracks), len(vias),
            lengths, max(lengths)-min(lengths) if lengths else 0,
            tuple(dict.fromkeys(diagnostics)), strategy="unified_final_review"))
    status = (CriticalRoutingStatus.FAILED if any(not item.connected for item in results)
              else CriticalRoutingStatus.SUCCESS)
    return CriticalRoutingResult(status, board, tuple(results), (), (),
        global_route.routing_fingerprint, detailed.routing_fingerprint)


def _plan_ordinary_escapes(board, fanout_options, on_progress):
    """Escapes for crowded ordinary pins, and keepouts that reserve them.

    Returns (tracks, vias, keepouts) on the macro-materialized frame, or None.
    """
    from collections import Counter
    from .fanout import route_fanout
    from .hard_macros import macro_source, materialize_hard_macros
    rules = {rule.net: rule for rule in board.net_routing_rules}
    # Signal nets only: supply and return nets are decoupled or planes, and
    # leaving them out also keeps the escape generator from routing bypasses.
    supply = {zone.net for zone in board.zones}
    supply |= {link.net for link in board.decoupling_links} | {link.return_net for link in board.decoupling_links}
    ordinary = frozenset(net.name for net in board.nets
                         if len(net.pads) >= 2 and net.name not in supply
                         and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL))
    if not ordinary:
        return None
    emit(on_progress, "ordinary_escapes", "started", nets=len(ordinary))
    frame = materialize_hard_macros(macro_source(board))
    try:
        escapes = route_fanout(frame, fanout_options, only_nets=ordinary)
    except ValueError as error:
        emit(on_progress, "ordinary_escapes", "finished", skipped=str(error))
        return None
    tracks = list((Counter(escapes.board.tracks) - Counter(frame.tracks)).elements())
    vias = list((Counter(escapes.board.vias) - Counter(frame.vias)).elements())
    keepouts = _copper_keepouts(board, tracks, vias, "escape-reserve", fixed=frame)
    emit(on_progress, "ordinary_escapes", "finished", vias=len(vias), tracks=len(tracks),
         pending=len(escapes.pending_pads))
    return tuple(tracks), tuple(vias), tuple(keepouts)


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


def _plan_decouplers(board, reserved_keepouts, on_progress):
    """Bypass copper routed before critical nets, held as keepouts meanwhile.

    The critical router needs a board without ordinary copper, so bypasses are
    planned on the macro-materialized frame (respecting escape reservations),
    reserved as keepouts while critical nets route, and added back afterwards.
    """
    from collections import Counter
    from .decoupling import route_decouplers
    from .hard_macros import macro_source, materialize_hard_macros
    emit(on_progress, "bypass_reservation", "started", links=len(board.decoupling_links))
    frame = materialize_hard_macros(macro_source(board))
    frame = replace(frame, copper_keepouts=(*frame.copper_keepouts, *reserved_keepouts))
    result = route_decouplers(frame)
    released = ()
    pending = frozenset(link for link, _ in result.pending)
    if pending and reserved_keepouts:
        # Escape reservations next to a blocked bypass give way to it: free
        # them around the link's own pads and route just those links again.
        pads = {pad for link in pending for pad in (link.capacitor, link.target, link.return_pad)}
        kept = _release_reservations(board, reserved_keepouts, frozenset(), nm_from_mm("1.5"), pads=pads)
        if len(kept) < len(reserved_keepouts):
            kept_ids = {k.id for k in kept}
            retry_frame = replace(result.board, copper_keepouts=tuple(
                k for k in result.board.copper_keepouts
                if not k.id.startswith("escape-reserve") or k.id in kept_ids))
            retry = route_decouplers(retry_frame, only_links=pending)
            if len(retry.pending) < len(pending):
                released = tuple(k for k in reserved_keepouts if k.id not in kept_ids)
                result = retry
    tracks = list((Counter(result.board.tracks) - Counter(frame.tracks)).elements())
    vias = list((Counter(result.board.vias) - Counter(frame.vias)).elements())
    emit(on_progress, "bypass_reservation", "finished", tracks=len(tracks), vias=len(vias),
         pending=len(result.pending), released=len(released))
    return (tuple(tracks), tuple(vias), _copper_keepouts(board, tracks, vias, "bypass-reserve", fixed=frame),
            released)


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
