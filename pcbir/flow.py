"""Compatible orchestration of placement feedback, routing, and signoff."""

from __future__ import annotations

import os
from dataclasses import dataclass, replace
from enum import Enum
from functools import partial

from .critical import CriticalRoutingResult, CriticalRoutingStatus, route_critical_nets
from .detailed import DetailedRoutingResult, DetailedRoutingStatus
from .drc import DrcDecision, PhysicalDrcPolicy, PhysicalDrcReport, run_physical_drc
from .fanout import FanoutOptions, FanoutResult
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .pad_stitch import DuplicatePadStitchResult
from .route_closure import close_detailed_lands
from .physical import PhysicalBoard, RouteKind, nm_from_mm
from .placement import PlacementPlannerOptions
from .routeflow import PlacementRoutingFeedbackOptions, PlacementRoutingResult, optimize_placement_for_routing
from .routing import GlobalRouterOptions
from .progress import ProgressCallback, critical_progress, emit


class PhysicalFlowStatus(str, Enum):
    PASS = "pass"
    FAIL = "fail"


# Stage boundaries where an iteration may stop early (``stop_after``). Stopping
# never passes a gate: the result is a FAIL with ``stopped_after`` set.
STOP_STAGES = ("placement",)


@dataclass(frozen=True, slots=True)
class RoutingPipelineResult:
    status: PhysicalFlowStatus
    placement_and_global: PlacementRoutingResult
    critical: CriticalRoutingResult
    detailed: DetailedRoutingResult
    drc: PhysicalDrcReport
    plane_stitch: PlaneStitchResult | None = None
    duplicate_pad_stitch: DuplicatePadStitchResult | None = None
    stopped_after: str | None = None
    # The router's options, for later transactional dependency repairs.
    unified_options: object | None = None
    # congestion_feedback.FeedbackOutcome of the placement feedback loop
    # (None when it was not enabled).
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
    drc_policy: PhysicalDrcPolicy | None = None,
    fanout_options: FanoutOptions | None = None,
    plane_stitch_options: PlaneStitchOptions | None = None,
    detailed_feedback_trials: int = 0,
    detailed_feedback_movement_nm: int = nm_from_mm("0.5"),
    critical_feedback_trials: int = 0,
    critical_pair_state_limit: int = 6_000,
    on_progress: ProgressCallback | None = None,
    unified_options=None,
    stop_after: str | None = None,
    initial_placement: PlacementRoutingResult | None = None,
    on_stage=None,
    escape_plan_options=None,
    placement_feedback_trials: int = 0,
) -> RoutingPipelineResult:
    """Run steps 4–8 in dependency order with the unified negotiated router.

    ``unified_options`` (default ``UnifiedRouterOptions()``) configure the
    router. ``fanout_options`` configure the escape generator, which seeds
    pad launches once (no gate, no search over alternatives); terminal
    access is the router's own, and power comes from declared planes.

    ``stop_after`` ends the run at a stage boundary (see ``STOP_STAGES``) with a
    FAIL result carrying ``stopped_after``. ``initial_placement`` replaces the
    placement/global stage with an earlier, explicitly supplied result.
    ``on_stage(name, value)`` observes completed stages (e.g. for checkpoints).

    ``critical_pair_state_limit`` caps each coupled pair's search in the
    critical stage (0: no cap); ``unified_options.critical_pair_state_limit``
    takes precedence when set.

    Placement feedback is the congestion-driven loop
    (``congestion_feedback.run_placement_feedback``) with a budget of
    ``max(placement_feedback_trials, detailed + critical)`` full reruns of
    the pipeline tail, started at ``detailed_feedback_movement_nm``.
    """
    from .unified_router import UnifiedRouterOptions
    unified_options = unified_options or UnifiedRouterOptions()
    unified_fanout = fanout_options
    route_ordinary = partial(_route_unified_compat, unified_options, escape_options=unified_fanout)

    from .hard_macros import macro_source
    board = macro_source(board)
    placement_options = placement_options or PlacementPlannerOptions()
    global_options = global_options or GlobalRouterOptions()
    feedback_options = feedback_options or PlacementRoutingFeedbackOptions(
        initial_movement_nm=global_options.tile_size_nm
    )
    if (detailed_feedback_trials < 0 or critical_feedback_trials < 0 or placement_feedback_trials < 0
            or detailed_feedback_movement_nm <= 0):
        raise ValueError("routing placement feedback bounds are invalid")
    if critical_pair_state_limit < 0:
        raise ValueError("critical pair state limit must be non-negative")
    pair_state_limit = unified_options.critical_pair_state_limit or critical_pair_state_limit or None
    if stop_after is not None and stop_after not in STOP_STAGES:
        raise ValueError(f"stop_after must be one of {', '.join(STOP_STAGES)}")
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
                                     stopped_after="placement", unified_options=unified_options)
    array_plans, surface_balls = _plan_array_escapes(placement.board, escape_plan_options, on_progress)
    if surface_balls is not None and unified_fanout is not None:
        unified_fanout = replace(unified_fanout, surface_pads=unified_fanout.surface_pads | surface_balls)
        route_ordinary = partial(_route_unified_compat, unified_options, escape_options=unified_fanout)
    lattice = _plan_lattice(placement.board, escape_plan_options, surface_balls, on_progress)
    if escape_plan_options is not None and escape_plan_options.two_end_order:
        # Plan once on the placed board: escape generation keeps kept nets'
        # surface pins off dogbone vias, and the router follows the plan's
        # order and layers.
        from .escape_plan import plan_two_end_escapes
        escape_plan = plan_two_end_escapes(placement.board, escape_plan_options)
        emit(on_progress, "escape_plan", "finished", bundles=len(escape_plan.pairs),
             planned_nets=len(escape_plan.net_order), surface_pads=len(escape_plan.surface_pads))
        unified_options = replace(unified_options, escape_net_order=escape_plan.net_order,
                                  escape_layer_biases=escape_plan.layer_biases)
        if unified_fanout is not None:
            unified_fanout = replace(unified_fanout,
                                     surface_pads=unified_fanout.surface_pads | escape_plan.surface_pads)
        route_ordinary = partial(_route_unified_compat, unified_options, escape_options=unified_fanout)
    reserved = held = None
    held_as_copper = False
    if unified_options.direct:
        critical = _direct_critical(placement)
        plane_stitch_options = None
        emit(on_progress, "unified_direct", "started",
             reason="all mutable nets negotiate together; only source-fixed copper is retained")
    else:
        critical, reserved, held, held_as_copper = _route_critical_stage(
            placement, unified_options=unified_options, unified_fanout=unified_fanout,
            pair_state_limit=pair_state_limit, plane_stitch_options=plane_stitch_options,
            on_progress=on_progress, on_stage=on_stage, lattice=lattice)
    critical, plane_stitch = _complete_critical_stage(
        critical, reserved, held, held_as_copper, unified_options=unified_options,
        plane_stitch_options=plane_stitch_options, on_progress=on_progress,
        lattice=lattice, array_plans=array_plans, escape_plan_options=escape_plan_options,
        save_inputs_path=unified_options.save_inputs_path, fanout_options=unified_fanout)
    pre_detailed = plane_stitch.board if plane_stitch else critical.board
    from .zone_geometry import deferred_zone_nets
    deferred_zones = frozenset(deferred_zone_nets(pre_detailed))
    critical, plane_stitch, pre_detailed = _release_escape_masks(
        reserved, critical, plane_stitch, pre_detailed, unified_options=unified_options,
        on_progress=on_progress)
    detailed, drc, duplicate_stitch = _route_ordinary_stage(
        route_ordinary, pre_detailed, placement.global_route, drc_policy=drc_policy, on_progress=on_progress)
    placement_feedback = None
    feedback_budget = max(placement_feedback_trials, detailed_feedback_trials + critical_feedback_trials)
    if feedback_budget:
        from .congestion_feedback import FeedbackState, run_placement_feedback
        # Each trial routes from its own placement, so it must not overwrite
        # the replay inputs of the run that is kept: trials save beside them,
        # and only the accepted trial's file replaces them (see below).
        inputs_path = unified_options.save_inputs_path
        trial_inputs: list[str] = []

        def rerun_tail(trial_board, trial_global):
            if inputs_path:
                trial_inputs.append(f"{inputs_path}.trial-{len(trial_inputs) + 1}")
                trial_route = partial(_route_unified_compat,
                                      replace(unified_options, save_inputs_path=trial_inputs[-1]),
                                      escape_options=unified_fanout)
            else:
                trial_route = route_ordinary
            # Exactly the main path's tail from the trial placement:
            # reservations, critical routing, held copper, islands, dogbones,
            # plane stitch, ordinary routing and closure.
            trial = replace(placement, board=trial_board, global_route=trial_global)
            # The escape frame (array plans, via lattice) of the trial placement.
            trial_plans, trial_surface = _plan_array_escapes(trial_board, escape_plan_options, on_progress)
            trial_lattice = _plan_lattice(trial_board, escape_plan_options, trial_surface, on_progress)
            if unified_options.direct:
                trial_critical, trial_reserved, trial_held, trial_as_copper = (
                    _direct_critical(trial), None, None, False)
            else:
                trial_critical, trial_reserved, trial_held, trial_as_copper = _route_critical_stage(
                    trial, unified_options=unified_options, unified_fanout=unified_fanout,
                    pair_state_limit=pair_state_limit, on_progress=on_progress, on_stage=None,
                    plane_stitch_options=plane_stitch_options, lattice=trial_lattice)
            trial_critical, trial_stitch = _complete_critical_stage(
                trial_critical, trial_reserved, trial_held, trial_as_copper,
                unified_options=unified_options, plane_stitch_options=plane_stitch_options,
                on_progress=on_progress, lattice=trial_lattice, array_plans=trial_plans,
                escape_plan_options=escape_plan_options,
                save_inputs_path=trial_inputs[-1] if inputs_path else None, fanout_options=unified_fanout)
            trial_pre = trial_stitch.board if trial_stitch else trial_critical.board
            trial_deferred = frozenset(deferred_zone_nets(trial_pre))
            trial_critical, trial_stitch, trial_pre = _release_escape_masks(
                trial_reserved, trial_critical, trial_stitch, trial_pre, unified_options=unified_options,
                on_progress=on_progress)
            trial_detailed, trial_drc, trial_duplicate = _route_ordinary_stage(
                trial_route, trial_pre, trial_global, drc_policy=drc_policy, on_progress=on_progress)
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
    if unified_options.direct:
        critical = _unified_profile_review(detailed, placement.global_route, drc)
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
        plane_stitch=plane_stitch,
        duplicate_pad_stitch=duplicate_stitch,
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


def _route_critical_stage(placement, *, unified_options, unified_fanout, pair_state_limit,
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
    planar = promote_planar_nets(board)[1]
    if planar:
        emit(on_progress, "planar_nets", "promoted", nets=sorted(planar))
    halos = _macro_port_halos(board)
    prefix = lattice[:2] if lattice else ((), ())
    if unified_fanout is not None and unified_options.escapes_first:
        # Ordinary pins of crowded packages get escapes planned before the
        # critical router runs; the critical router sees them as keepouts,
        # so it cannot wall those pins in (one exact pass, no gate).
        reserved = _plan_ordinary_escapes(board, unified_fanout, on_progress, prefix=prefix,
                                          keepouts=(*halos, *_plane_pin_halos(board)), exclude=planar)
    if board.decoupling_links and unified_options.pre_route_decouplers:
        # Bypasses route before critical nets (as package access did), so
        # a critical route cannot take a capacitor's short surface feed.
        held, reserved = _plan_decouplers(board, reserved, on_progress, prefix=prefix, extra_keepouts=halos,
                                          fanout_options=unified_fanout)
        if reserved is not None and held[3]:
            reserved, held, restored = _revalidate_released_escapes(board, reserved, held)
            emit(on_progress, "ordinary_escapes", "revalidated", **restored)
            if restored.get("unrestored_nets") and unified_fanout is not None:
                reserved = _resite_escapes(board, unified_fanout, reserved, held, prefix,
                                           (*halos, *_plane_pin_halos(board)),
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
    return_options = critical_return_plane_options(source, plane_stitch_options, halos)
    if on_stage is not None:
        on_stage("critical-inputs", {"board": source, "global_route": placement.global_route,
                                     "reserved_accesses": bypass_accesses,
                                     "plane_stitch_options": return_options})
    critical = route_critical_nets(source, placement.global_route,
        reserved_accesses=bypass_accesses, pair_state_limit=pair_state_limit,
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
                if unified_options.retain_escape_copper:
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
                    reserved_accesses=retry_accesses, pair_state_limit=pair_state_limit,
                    plane_stitch_options=return_options,
                    on_progress=critical_progress(on_progress), unified_options=unified_options)
                    if retry_source is not None else None)
                if retry is not None and sum(not item.connected for item in retry.nets) < len(
                        [item for item in critical.nets if not item.connected]):
                    critical = retry
                    reserved = (reserved[0], reserved[1], kept)
                    held_as_copper = retry_accesses is not None
    return critical, reserved, held, held_as_copper


def _plane_pin_halos(board):
    """Keepouts in front of fine-pitch plane pins, for ordinary escape planning.

    A pin of a distribution zone net without a zone on its own layer reaches
    its plane through a stitching via placed after routing. Ordinary escapes
    are planned first and may put a neighbouring pin's via or jog right in
    front of it, so the stitch later finds the pin walled in. The halo keeps
    one via site free: from one clearance past the land's outer end, a via
    and a clearance outward, one via plus clearance wide, on the land's
    layer. Outward is along the land's long axis, away from the package
    centre; lands that are not at least 1.5 times longer than wide have no
    such axis and get none. A halo is only set where the neighbouring pins'
    straight escapes (a default track) still pass beside it, so pitches too
    fine for a via in front of a pin, and pins farther than 0.65 mm from
    their nearest neighbour, are left alone.
    """
    from .drc import placed_pad_shape
    from .physical import BoardSide, CopperKeepout, CopperLayer, PadKind, Point, PolygonRing, PolygonWithHoles
    from .placement import transformed_local_point
    from .zone_geometry import distribution_zone_nets
    zone_nets = distribution_zone_nets(board)
    if not zone_nets:
        return ()
    zone_layers = {}
    for zone in board.zones:
        zone_layers.setdefault(zone.net, set()).update(zone.layers)
    gap = board.rules.minimum_clearance_nm
    via = board.rules.default_via_size_nm
    half = via // 2 + gap
    track = board.rules.default_track_width_nm
    nets = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
    halos = []
    for placement in board.placements:
        footprint = board.footprints[placement.footprint]
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        centres = {pad.number: transformed_local_point(placement, pad.position) for pad in footprint.pads}
        for pad in footprint.pads:
            net = nets.get((placement.reference, pad.number))
            if (net not in zone_nets or side in zone_layers[net] or pad.kind is not PadKind.SMD):
                continue
            centre = centres[pad.number]
            bounds = placed_pad_shape(centre, pad, placement).bounds
            width, height = bounds.max_x - bounds.min_x, bounds.max_y - bounds.min_y
            if 2 * max(width, height) < 3 * min(width, height):
                continue
            dx, dy = centre.x_nm - placement.position.x_nm, centre.y_nm - placement.position.y_nm
            ox, oy = ((1 if dx > 0 else -1), 0) if width > height else (0, (1 if dy > 0 else -1))
            if (ox and not dx) or (oy and not dy):
                continue
            # Nearest neighbouring land across the outward axis.
            across = [abs(point.y_nm - centre.y_nm) if ox else abs(point.x_nm - centre.x_nm)
                      for other, point in centres.items() if other != pad.number
                      and abs(point.x_nm - centre.x_nm) + abs(point.y_nm - centre.y_nm) <= nm_from_mm("0.65")]
            if not across or half + gap + (track + 1) // 2 > min(across):
                continue
            edge = (bounds.max_x if ox > 0 else bounds.min_x) if ox else (bounds.max_y if oy > 0 else bounds.min_y)
            near, far = edge + (ox or oy) * gap, edge + (ox or oy) * (2 * gap + via)
            if ox:
                xs, ys = sorted((near, far)), (centre.y_nm - half, centre.y_nm + half)
            else:
                xs, ys = (centre.x_nm - half, centre.x_nm + half), sorted((near, far))
            ring = PolygonRing((Point(xs[0], ys[0]), Point(xs[1], ys[0]), Point(xs[1], ys[1]), Point(xs[0], ys[1])))
            halos.append(CopperKeepout(f"plane-pin-halo-{len(halos)}", (side,), PolygonWithHoles(ring),
                                       block_tracks=True, block_vias=True, block_zones=False))
    return tuple(halos)


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


def _complete_critical_stage(critical, reserved, held, held_as_copper, *, unified_options,
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
    if (critical.board.decoupling_links and not unified_options.direct
            and unified_options.pre_route_decouplers):
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
    if unified_options.power_islands:
        from .physical import CopperLayer
        from .power_islands import auto_power_layer, plan_power_islands
        choice = unified_options.power_islands
        layer = auto_power_layer(critical.board) if choice == "auto" else CopperLayer(choice)
        if layer is not None:
            emit(on_progress, "power_islands", "started", layer=layer.value)
            if unified_options.power_islands_ball_grid_only:
                from .power_islands import plan_ball_grid_islands
                partition = plan_ball_grid_islands(critical.board, layer)
            else:
                partition = plan_power_islands(critical.board, layer)
            critical = replace(critical, board=replace(
                critical.board, zones=(*critical.board.zones, *partition.zones)))
            emit(on_progress, "power_islands", "finished", **partition.report())
    if unified_options.bga_fanout:
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
    plane_stitch = stitch_zone_pads(critical.board, plane_stitch_options) if plane_stitch_options else None
    return critical, plane_stitch


def _release_escape_masks(reserved, critical, plane_stitch, pre_detailed, *, unified_options=None,
                          on_progress=None):
    """Drop the temporary escape-reservation keepouts before ordinary routing.

    With ``unified_options.planned_escapes_as_copper`` the promised exits
    join the ordinary stage's board as copper (each piece exact-checked, a
    piece the preparation steps made illegal is left out and reported):
    the stage's fanout then keeps them instead of finding exits again on a
    board that critical copper, bypasses and plane contacts have filled.
    """
    if reserved is None:
        return critical, plane_stitch, pre_detailed
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
    pre_detailed = release_masks(pre_detailed)
    if getattr(unified_options, "planned_escapes_as_copper", False) and (reserved[0] or reserved[1]):
        from .track_containment import track_is_contained
        joined = _join_reserved_copper(pre_detailed, reserved[0], reserved[1], ())
        kept_tracks = joined.tracks[len(pre_detailed.tracks):]
        kept_vias = joined.vias[len(pre_detailed.vias):]
        # A piece the board already held (a dogbone another step placed on
        # the promised site) is reused, not dropped.
        present = {(v.net, v.position, v.from_layer, v.to_layer) for v in pre_detailed.vias}
        reused_vias = [v for v in reserved[1] if (v.net, v.position, v.from_layer, v.to_layer) in present]
        reused_tracks = [t for t in reserved[0] if track_is_contained(t, pre_detailed.tracks)]
        dropped_vias = [v for v in reserved[1] if v not in kept_vias and v not in reused_vias]
        dropped_tracks = [t for t in reserved[0] if t not in kept_tracks and t not in reused_tracks]
        emit(on_progress, "ordinary_escapes", "joined", tracks=len(kept_tracks), vias=len(kept_vias),
             reused_tracks=len(reused_tracks), reused_vias=len(reused_vias),
             dropped_tracks=len(dropped_tracks), dropped_vias=len(dropped_vias),
             dropped_nets=sorted({item.net for item in (*dropped_tracks, *dropped_vias)}))
        pre_detailed = joined
    return critical, plane_stitch, pre_detailed


def _route_ordinary_stage(route_ordinary, pre_detailed, global_route, *, drc_policy, on_progress):
    """Ordinary routing then land closure and DRC; returns ``(detailed, drc, duplicate_stitch)``."""
    emit(on_progress, "ordinary_area", "started")
    detailed = route_ordinary(pre_detailed, global_route, on_progress=on_progress)
    emit(on_progress, "ordinary_area", "finished", status=detailed.status.value,
         failed_nets=[n.net for n in detailed.nets if not n.connected],
         passes=detailed.metrics.passes)
    emit(on_progress, "land_closure_native_drc", "started")
    detailed, drc, duplicate_stitch = close_detailed_lands(detailed, drc_policy)
    emit(on_progress, "land_closure_native_drc", "finished", decision=drc.decision.value)
    return detailed, drc, duplicate_stitch


def _unified_profile_review(detailed, global_route, drc):
    """Report final profile copper without calling another routing solver.

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
    from .track_containment import track_is_contained
    board = replace(board, copper_keepouts=tuple(k for k in board.copper_keepouts if k.id not in temporary))
    index = RoutingClearanceIndex(board)
    kept_tracks, kept_vias = [], []
    present = {(v.net, v.position, v.from_layer, v.to_layer) for v in board.vias}
    for via in vias:
        if (via.net, via.position, via.from_layer, via.to_layer) in present:
            continue  # The board already holds this exit: reuse it, no second drill.
        if index.can_via(via.net, via.position, via.size_nm, via.from_layer, via.to_layer,
                         drill_nm=via.drill_nm):
            index.add_via(via)
            kept_vias.append(via)
            present.add((via.net, via.position, via.from_layer, via.to_layer))
    for track in tracks:
        if track_is_contained(track, board.tracks):
            continue
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


def _route_unified_compat(unified_options, board, global_route, *, escape_options=None, **kwargs):
    from .unified_router import route_unified
    seeder = None
    if escape_options is not None and not unified_options.direct:
        from .fanout import route_fanout

        def seeder(partial, nets):
            return route_fanout(partial, escape_options, only_nets=nets).board
    return route_unified(board, global_route, unified_options, escape_seeder=seeder, **kwargs)


def _feedback_score(critical, detailed: DetailedRoutingResult, drc: PhysicalDrcReport) -> tuple[int, ...]:
    """Acceptance tuple of the unified placement feedback loop; lower is better.

    ``(hard DRC errors, open signal nets (DRC-OPEN-NET, deferred zone nets
    excluded), critical groups not connected or with diagnostics, unrouted
    ordinary nets, conflict overflow, total length, vias)``; see
    ``congestion_feedback.SCORE_ORDER``.
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
