"""Transactional package-escape and placement feedback for zone-net pads.

Only a small set of late-failing pads is reserved early. Each candidate starts
from an unrouted placement. Incremental transactions rebuild affected copper
and revalidate preserved copper; unsupported candidates use the full pipeline.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal
from collections import Counter
from hashlib import sha256
from math import hypot

from .critical import CriticalRoutingStatus
from .clusters import move_placement_unit
from .detailed import DetailedRouterOptions, DetailedRoutingResult, DetailedRoutingStatus, route_detailed
from .drc import DrcDecision, PhysicalDrcPolicy, run_physical_drc
from .fanout import FanoutOptions
from .package_access import PackageAccessOptions
from .flow import PhysicalFlowStatus, RoutingPipelineResult, run_routing_pipeline
from .physical import PadReference, PhysicalBoard, Placement, Point, TrackSegment, Via, nm_from_mm
from .placement import PlacementPlannerOptions, placement_solution_is_legal
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .routeflow import PlacementRoutingFeedbackOptions
from .routing import GlobalRouterOptions, GlobalRoutingStatus
from .routing_clearance import RoutingClearanceIndex
from .route_closure import close_detailed_lands
from .progress import ProgressCallback, emit


@dataclass(frozen=True, slots=True)
class EscapeFeedbackOptions:
    maximum_trials: int = 4
    maximum_local_trials: int = 6
    maximum_local_blockers: int = 4
    movement_nm: int = nm_from_mm("0.5")
    nearby_components: int = 3
    maximum_dependency_expansions: int = 2
    incremental_placement: bool = True

    def __post_init__(self) -> None:
        if (self.maximum_trials < 0 or self.maximum_local_trials < 0
                or not 1 <= self.maximum_local_blockers <= 8
                or not 0 <= self.maximum_dependency_expansions <= 8
                or self.movement_nm <= 0 or self.nearby_components < 0):
            raise ValueError("escape feedback bounds are invalid")


@dataclass(frozen=True, slots=True)
class EscapeFeedbackAttempt:
    description: str
    early_pending: tuple[PadReference, ...]
    late_pending: tuple[PadReference, ...] | None
    signal_failures: int | None
    accepted: bool
    decision: str
    failed_signals: tuple[str, ...] = ()
    repair_nets: tuple[str, ...] = ()
    dependency_expansions: int = 0
    strategy: str = "full_pipeline"
    changed_references: tuple[str, ...] = ()
    rebuilt_zone_nets: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class EscapeFeedbackResult:
    pipeline: RoutingPipelineResult
    plane_stitch: PlaneStitchResult
    attempts: tuple[EscapeFeedbackAttempt, ...]


def improve_zone_escapes(
    initial: RoutingPipelineResult,
    plane_options: PlaneStitchOptions,
    *,
    placement_options: PlacementPlannerOptions | None = None,
    global_options: GlobalRouterOptions | None = None,
    feedback_options: PlacementRoutingFeedbackOptions | None = None,
    detailed_options: DetailedRouterOptions | None = None,
    drc_policy: PhysicalDrcPolicy | None = None,
    fanout_options: FanoutOptions | None = None,
    package_access_options: PackageAccessOptions | None = None,
    options: EscapeFeedbackOptions | None = None,
    _reserved_pads: frozenset[PadReference] = frozenset(),
    on_progress: ProgressCallback | None = None,
) -> EscapeFeedbackResult:
    """Repair late pad escapes without sacrificing completed signal routes."""

    options = options or EscapeFeedbackOptions()
    placement_options = placement_options or PlacementPlannerOptions()
    baseline_stitch = stitch_zone_pads(initial.board, plane_options)
    emit(on_progress, "zone_contacts", "finished",
         pending_pads=[f"{p.component}.{p.pad}" for p in baseline_stitch.pending_pads])
    if initial.package_access is not None and not initial.package_access.ready:
        # A late-plane repair cannot bypass the earlier package-access gate.
        return EscapeFeedbackResult(initial, baseline_stitch, ())
    if not baseline_stitch.pending_pads:
        return EscapeFeedbackResult(initial, baseline_stitch, ())
    if options.maximum_local_trials:
        emit(on_progress, "zone_local_repair", "started")
        local = _repair_with_local_ripup(
            initial, baseline_stitch, plane_options,
            detailed_options or DetailedRouterOptions(), options, on_progress=on_progress,
        )
        emit(on_progress, "zone_local_repair", "finished", improved=local is not None)
        if local is not None:
            if local.plane_stitch.complete or options.maximum_trials == 0:
                return local
            continuation = improve_zone_escapes(
                local.pipeline, plane_options,
                placement_options=placement_options,
                global_options=global_options,
                feedback_options=feedback_options,
                detailed_options=detailed_options,
                drc_policy=drc_policy,
                fanout_options=fanout_options,
                package_access_options=package_access_options,
                options=replace(options, maximum_local_trials=0),
                _reserved_pads=_reserved_pads,
                on_progress=on_progress,
            )
            return EscapeFeedbackResult(
                continuation.pipeline, continuation.plane_stitch,
                (*local.attempts, *continuation.attempts),
            )
    if options.maximum_trials == 0:
        return EscapeFeedbackResult(initial, baseline_stitch, ())
    targets = frozenset(baseline_stitch.pending_pads)
    # A late retry must not abandon contacts accepted by the package gate.
    # Keep every previous owner's identity when rebuilding the unrouted scene,
    # not just the currently failing package's ground group.
    if initial.plane_stitch is not None:
        _reserved_pads = _reserved_pads | frozenset(initial.plane_stitch.stitched_pads)
    package_targets = _package_zone_targets(initial.board, targets)
    zone_nets = {zone.net for zone in initial.board.zones}
    baseline_signal_failures = frozenset(_failed_signals(initial, zone_nets))
    baseline_hard = _hard_drc_findings(baseline_stitch.board)
    accepted_pending = len(baseline_stitch.pending_pads)
    base = initial.placement_and_global.board
    attempts: list[EscapeFeedbackAttempt] = []
    seen_copper: set[tuple[object, ...]] = set()
    routed_trials = 0
    for trial_board, bias, description in _candidate_placements(
        base, targets, placement_options, options,
    ):
        early_options = replace(
            plane_options, only_pads=package_targets | _reserved_pads,
            candidate_bias=bias,
        )
        preflight = stitch_zone_pads(trial_board, early_options)
        signature = (
            tuple(trial_board.placements),
            tuple(preflight.pending_pads),
            tuple(preflight.board.tracks), tuple(preflight.board.vias),
        )
        if signature in seen_copper:
            continue
        seen_copper.add(signature)
        pending_early = frozenset(preflight.pending_pads)
        if (pending_early & _reserved_pads
                or targets.issubset(pending_early)):
            attempts.append(EscapeFeedbackAttempt(
                description, preflight.pending_pads, None, None, False,
                "no legal early pad exit",
                strategy="early_screen",
            ))
            continue
        fixed = replace(
            placement_options,
            candidate_count=1,
            analytical_iterations=0,
            refinement_passes=0,
            fixed_references=frozenset(
                item.reference for item in trial_board.placements
            ),
        )
        incremental = None
        if options.incremental_placement and trial_board.placements != initial.board.placements:
            from .incremental_placement import repair_placement_trial
            emit(on_progress, "zone_incremental_trial", "started", index=routed_trials + 1,
                 description=description)
            incremental = repair_placement_trial(initial, trial_board, baseline_stitch,
                early_options, plane_options, options=options,
                detailed_options=detailed_options or DetailedRouterOptions(),
                placement_options=placement_options, global_options=global_options,
                fanout_options=fanout_options, drc_policy=drc_policy, on_progress=on_progress)
            if incremental is None:
                emit(on_progress, "zone_incremental_trial", "finished", index=routed_trials + 1,
                     description=description,
                     decision="fallback to full pipeline")
        phase = "zone_incremental_trial" if incremental else "zone_full_trial"
        if incremental is None:
            emit(on_progress, phase, "started", index=routed_trials + 1, description=description)
            trial_pipeline = run_routing_pipeline(
                trial_board,
                placement_options=fixed,
                global_options=global_options,
                feedback_options=replace(
                    feedback_options or PlacementRoutingFeedbackOptions(),
                    preferred_candidate_id=None, maximum_iterations=1,
                ),
                detailed_options=detailed_options,
                drc_policy=drc_policy,
                fanout_options=fanout_options,
                package_access_options=package_access_options,
                plane_stitch_options=early_options,
                on_progress=on_progress,
            )
        else:
            trial_pipeline = incremental.pipeline
        routed_trials += 1
        late = incremental.plane_stitch if incremental else stitch_zone_pads(trial_pipeline.board, plane_options)
        failed_signals = _failed_signals(trial_pipeline, zone_nets)
        signal_failures = len(failed_signals)
        hard = _hard_drc_findings(late.board)
        accepted = (
            trial_pipeline.placement_and_global.global_route.status
            is GlobalRoutingStatus.SUCCESS
            and trial_pipeline.critical.status is not CriticalRoutingStatus.FAILED
            and hard <= baseline_hard
            and set(failed_signals).issubset(baseline_signal_failures)
            and len(late.pending_pads) < accepted_pending
        )
        if accepted:
            decision = "accepted: fewer pending zone pads"
        elif trial_pipeline.placement_and_global.global_route.status is not GlobalRoutingStatus.SUCCESS:
            decision = "rejected: global routing regressed"
        elif trial_pipeline.critical.status is CriticalRoutingStatus.FAILED:
            decision = "rejected: critical routing failed"
        elif hard > baseline_hard:
            decision = "rejected: hard DRC regressed"
        elif not set(failed_signals).issubset(baseline_signal_failures):
            decision = "rejected: signal routing regressed"
        else:
            decision = "rejected: zone pad count did not improve"
        attempts.append(EscapeFeedbackAttempt(
            description, preflight.pending_pads, late.pending_pads,
            signal_failures, accepted, decision, failed_signals,
            repair_nets=incremental.repair_nets if incremental else (),
            dependency_expansions=incremental.dependency_expansions if incremental else 0,
            strategy="incremental_placement" if incremental else "full_pipeline",
            changed_references=incremental.changed_references if incremental else (),
            rebuilt_zone_nets=incremental.rebuilt_zone_nets if incremental else (),
        ))
        emit(on_progress, phase, "finished", index=routed_trials,
             description=description, decision=decision,
             pending_pads=[f"{p.component}.{p.pad}" for p in late.pending_pads],
             failed_signals=list(failed_signals))
        if accepted:
            accepted_pending = len(late.pending_pads)
            baseline_hard = hard
            if (accepted_pending
                    and routed_trials < options.maximum_trials):
                continuation = improve_zone_escapes(
                    trial_pipeline, plane_options,
                    placement_options=placement_options,
                    global_options=global_options,
                    feedback_options=feedback_options,
                    detailed_options=detailed_options,
                    drc_policy=drc_policy,
                    fanout_options=fanout_options,
                    package_access_options=package_access_options,
                    options=replace(
                        options,
                        maximum_trials=options.maximum_trials-routed_trials,
                    ),
                    _reserved_pads=(
                        _reserved_pads | (package_targets - pending_early)
                    ),
                    on_progress=on_progress,
                )
                return EscapeFeedbackResult(
                    continuation.pipeline, continuation.plane_stitch,
                    (*attempts, *continuation.attempts),
                )
            return EscapeFeedbackResult(
                trial_pipeline, late, tuple(attempts),
            )
        if routed_trials >= options.maximum_trials:
            break
    return EscapeFeedbackResult(initial, baseline_stitch, tuple(attempts))


def _package_zone_targets(
    board: PhysicalBoard, pending: frozenset[PadReference],
) -> frozenset[PadReference]:
    """Reserve a failing package's entire same-zone-net pin group together.

    A previously escaped neighbour pin must not lose its exit during a trial
    that repairs another pin on the same package. Other packages/rails are not
    indiscriminately reserved. Reservation precedes fanout/ordinary routing;
    final native and filled-zone checks still decide whether the trial passes.
    """
    zone_nets = {zone.net for zone in board.zones}
    groups = {(net.name, pad.component) for net in board.nets if net.name in zone_nets
              for pad in net.pads if pad in pending}
    return pending | frozenset(pad for net in board.nets for pad in net.pads
                              if (net.name, pad.component) in groups)


def _repair_with_local_ripup(
    initial: RoutingPipelineResult,
    baseline: PlaneStitchResult,
    plane_options: PlaneStitchOptions,
    detailed_options: DetailedRouterOptions,
    options: EscapeFeedbackOptions,
    *, on_progress: ProgressCallback | None = None,
) -> EscapeFeedbackResult | None:
    """Try an exact pad escape, displacing only the signal nets it intersects.

    Remove *only* copper created by the detailed router to discover an exit.
    Critical routes, fanout, pads, keepouts and earlier plane contacts stay
    immutable. The trial is committed only after its blockers reroute and the
    complete board passes the same hard-DRC comparison as placement feedback.
    """

    pre_detail = (initial.fanout.board if initial.fanout is not None else
                  initial.plane_stitch.board if initial.plane_stitch is not None else
                  initial.critical.board)
    locked_tracks = Counter(pre_detail.tracks)
    locked_vias = Counter(pre_detail.vias)
    fixed_tracks = []
    movable_tracks = []
    fixed_vias = []
    movable_vias = []
    for track in initial.board.tracks:
        if locked_tracks[track]:
            fixed_tracks.append(track)
            locked_tracks[track] -= 1
        else:
            movable_tracks.append(track)
    for via in initial.board.vias:
        if locked_vias[via]:
            fixed_vias.append(via)
            locked_vias[via] -= 1
        else:
            movable_vias.append(via)
    zone_nets = {zone.net for zone in initial.board.zones}
    completed_signals = {
        item.net for item in initial.detailed.nets
        if item.connected and item.net not in zone_nets
    }
    if not completed_signals:
        return None
    # The ordinary plane stitch is appended after all detailed copper.
    plane_tracks = baseline.board.tracks[len(initial.board.tracks):]
    plane_vias = baseline.board.vias[len(initial.board.vias):]
    fixed = replace(
        initial.board,
        tracks=tuple((*fixed_tracks, *plane_tracks)),
        vias=tuple((*fixed_vias, *plane_vias)),
    )
    baseline_hard = _hard_drc_findings(baseline.board)
    baseline_failures = set(_failed_signals(initial, zone_nets))
    full_clearance = RoutingClearanceIndex(fixed)
    for track in movable_tracks:
        full_clearance.add_track(track)
    for via in movable_vias:
        full_clearance.add_via(via)
    trials = 0
    for target in baseline.pending_pads:
        if trials >= options.maximum_local_trials:
            break
        # Try nearby legal exit directions with *only* movable detailed
        # copper removed. Exact clearance queries below reveal which of
        # those routes are actually in the way; no net is ripped up merely
        # because it happens to be spatially close to the pad.
        biases = (plane_options.candidate_bias, "south", "north", "east", "west")
        for bias in dict.fromkeys(biases):
            if trials >= options.maximum_local_trials:
                break
            trials += 1
            candidate = stitch_zone_pads(
                fixed, replace(
                    plane_options, only_pads=frozenset({target}),
                    candidate_bias=bias,
                ),
            )
            if target in candidate.pending_pads:
                continue
            new_tracks = candidate.board.tracks[len(fixed.tracks):]
            new_vias = candidate.board.vias[len(fixed.vias):]
            # A tentative exit must not cross another signal or a locked
            # object. The final reroute below checks exact geometry again.
            blockers = set()
            immutable = False
            for track in new_tracks:
                names, locked = full_clearance.blocking_track_nets(track)
                blockers.update(names)
                immutable |= locked
            for via in new_vias:
                names, locked = full_clearance.blocking_via_nets(via)
                blockers.update(names)
                immutable |= locked
            if (immutable or not blockers.issubset(completed_signals)
                    or len(blockers) > options.maximum_local_blockers):
                continue
            displaced = frozenset(blockers)
            repaired = _reroute_local_dependencies(
                initial, candidate.board, tuple(movable_tracks), tuple(movable_vias),
                displaced, frozenset(completed_signals), detailed_options, options,
                on_progress=on_progress,
            )
            if repaired is None:
                continue
            routed_board, reroute, displaced, expansions = repaired
            trial_board = _with_preserved_area(
                candidate.board, movable_tracks, movable_vias, displaced)
            # Subset routing is a transaction: copper belonging to every
            # unaffected net must survive exactly, not merely retain an old flag.
            if (_unaffected_copper(routed_board, displaced)
                    != _unaffected_copper(trial_board, displaced)):
                continue
            late = stitch_zone_pads(routed_board, plane_options)
            if (len(late.pending_pads) >= len(baseline.pending_pads)
                    or _hard_drc_findings(late.board) > baseline_hard):
                continue
            # Report total newly committed plane copper, including the
            # baseline contacts installed before this local transaction.
            late = replace(
                late,
                added_track_count=(
                    sum(track.net in zone_nets for track in late.board.tracks)
                    - sum(track.net in zone_nets for track in initial.board.tracks)
                ),
                added_via_count=(
                    sum(via.net in zone_nets for via in late.board.vias)
                    - sum(via.net in zone_nets for via in initial.board.vias)
                ),
            )
            merged = _merge_local_detail(
                initial, routed_board, reroute, displaced,
            )
            failed = set(_failed_signals(merged, zone_nets))
            if not failed.issubset(baseline_failures):
                continue
            attempt = EscapeFeedbackAttempt(
                f"local rip-up {target.component}.{target.pad}: "
                + (", ".join(sorted(displaced)) if displaced else "no signal blocker"),
                baseline.pending_pads, late.pending_pads, len(failed), True,
                "accepted: local shield/plane escape with displaced signals rerouted",
                tuple(sorted(failed)),
                tuple(sorted(displaced)), expansions,
                "local_dependency",
            )
            return EscapeFeedbackResult(merged, late, (attempt,))
    return None


def _with_preserved_area(
    fixed: PhysicalBoard, tracks: tuple[TrackSegment, ...] | list[TrackSegment],
    vias: tuple[Via, ...] | list[Via], changed: frozenset[str],
) -> PhysicalBoard:
    return replace(fixed,
        tracks=(*fixed.tracks, *(track for track in tracks if track.net not in changed)),
        vias=(*fixed.vias, *(via for via in vias if via.net not in changed)))


def _reroute_local_dependencies(
    initial: RoutingPipelineResult, fixed: PhysicalBoard,
    movable_tracks: tuple[TrackSegment, ...], movable_vias: tuple[Via, ...],
    changed: frozenset[str], eligible: frozenset[str], detailed_options: DetailedRouterOptions,
    options: EscapeFeedbackOptions, *, on_progress: ProgressCallback | None = None,
) -> tuple[PhysicalBoard, DetailedRoutingResult | None, frozenset[str], int] | None:
    """Expand an exact blocker cone, never accepting relaxed probe geometry.

    The fixed prefix owns critical copper, fanout and accepted ground contacts.
    Placement is unchanged, so global/access evidence remains valid. Every actual
    retry starts afresh with all unrelated area copper restored. Only completed
    ordinary nets may join the bounded cone; failed probes fall back to the caller.
    """
    if not changed.issubset(eligible) or len(changed) > options.maximum_local_blockers:
        return None
    if not changed:
        return _with_preserved_area(fixed, movable_tracks, movable_vias, changed), None, changed, 0

    def search(source, names, kind, expansion):
        emit(on_progress, "zone_subset_search", "started", kind=kind,
             affected_nets=sorted(names), expansion=expansion)
        result = route_detailed(source, initial.placement_and_global.global_route,
            detailed_options, fanout_accesses=initial.fanout.routing_accesses if initial.fanout else None,
            # Local transactions do not own/prune the fixed fanout prefix.
            fanout_created_vias=frozenset(), fanout_created_tracks=(), only_nets=names,
            on_progress=on_progress)
        result, _, _ = close_detailed_lands(result)
        emit(on_progress, "zone_subset_search", "finished", kind=kind,
             affected_nets=sorted(names), expansion=expansion,
             overflow=result.metrics.total_conflict_overflow,
             failed_nets=[item.net for item in result.nets if not item.connected])
        return result

    def complete(result, names):
        return ({item.net for item in result.nets} == set(names)
                and all(item.connected for item in result.nets)
                and result.metrics.total_conflict_overflow == 0)

    for expansion in range(options.maximum_dependency_expansions + 1):
        trial = _with_preserved_area(fixed, movable_tracks, movable_vias, changed)
        actual = search(trial, changed, "transaction", expansion)
        if complete(actual, changed):
            # Even changed-net fanout/critical contacts remain owned by the prefix.
            if (Counter(fixed.tracks) - Counter(actual.board.tracks)
                    or Counter(fixed.vias) - Counter(actual.board.vias)
                    or _unaffected_copper(actual.board, changed) != _unaffected_copper(trial, changed)):
                return None
            return actual.board, actual, changed, expansion
        if expansion == options.maximum_dependency_expansions:
            return None
        probe = search(fixed, changed, "probe_only", expansion)
        if not complete(probe, changed):
            return None
        # Probe paths are proposals only. Ask the exact index which preserved
        # tracks/vias they intersect, including through-via holes/all copper layers.
        clearance = RoutingClearanceIndex(fixed)
        for track in movable_tracks:
            if track.net not in changed:
                clearance.add_track(track)
        for via in movable_vias:
            if via.net not in changed:
                clearance.add_via(via)
        blockers = set()
        locked = False
        new_tracks = Counter(probe.board.tracks) - Counter(fixed.tracks)
        new_vias = Counter(probe.board.vias) - Counter(fixed.vias)
        for objects, query in ((new_tracks, clearance.blocking_track_nets),
                               (new_vias, clearance.blocking_via_nets)):
            for item in objects:
                if item.net not in changed:
                    continue
                names, immutable = query(item)
                blockers.update(names)
                locked |= immutable
        enlarged = changed | frozenset(blockers)
        if (locked or enlarged == changed or not enlarged.issubset(eligible)
                or len(enlarged) > options.maximum_local_blockers):
            return None
        changed = enlarged
    return None


def _merge_local_detail(
    original: RoutingPipelineResult,
    board: PhysicalBoard,
    reroute: DetailedRoutingResult | None,
    changed: frozenset[str],
    *, drc_policy: PhysicalDrcPolicy | None = None,
) -> RoutingPipelineResult:
    """Preserve full-board metrics while replacing a bounded net subset."""

    new = {item.net: item for item in reroute.nets} if reroute is not None else {}
    drc = run_physical_drc(board, policy=drc_policy)
    opens = {net for finding in drc.findings if finding.code == "DRC-OPEN-NET"
             for net in finding.nets}
    locked = (original.fanout.board if original.fanout else
              original.plane_stitch.board if original.plane_stitch else
              original.critical.board)
    zone_nets = {zone.net for zone in board.zones}
    tracks = Counter(track for track in board.tracks if track.net not in zone_nets)
    tracks -= Counter(locked.tracks)
    vias = Counter(via for via in board.vias if via.net not in zone_nets)
    vias -= Counter(locked.vias)
    nets = tuple(replace(
        new.get(item.net, item),
        connected=new.get(item.net, item).connected and item.net not in opens,
        track_count=sum(count for track, count in tracks.items() if track.net == item.net),
        via_count=sum(count for via, count in vias.items() if via.net == item.net),
        length_nm=sum(round(hypot(track.end.x_nm-track.start.x_nm,
                                 track.end.y_nm-track.start.y_nm)) * count
                      for track, count in tracks.items() if track.net == item.net),
    ) for item in original.detailed.nets)
    metrics = replace(
        original.detailed.metrics,
        routed_net_count=sum(item.connected for item in nets),
        unrouted_net_count=sum(not item.connected for item in nets),
        track_count=sum(item.track_count for item in nets),
        via_count=sum(item.via_count for item in nets),
        total_length_nm=sum(item.length_nm for item in nets),
    )
    fingerprint = sha256(repr((
        original.detailed.routing_fingerprint, board.tracks,
        board.vias,
    )).encode()).hexdigest()
    detailed = replace(
        original.detailed, board=board, nets=nets, metrics=metrics,
        routing_fingerprint=fingerprint,
    )
    detailed, drc, duplicate_stitch = close_detailed_lands(detailed, policy=drc_policy)
    passed = (original.placement_and_global.full_route_certified
              and original.critical.status is not CriticalRoutingStatus.FAILED
              and detailed.status is DetailedRoutingStatus.SUCCESS
              and drc.decision is DrcDecision.PASS)
    return replace(
        original, detailed=detailed,
        drc=drc,
        status=PhysicalFlowStatus.PASS if passed else PhysicalFlowStatus.FAIL,
        duplicate_pad_stitch=duplicate_stitch,
    )


def _failed_signals(
    pipeline: RoutingPipelineResult, zone_nets: set[str],
) -> tuple[str, ...]:
    reported = {net.net for net in pipeline.detailed.nets
                if net.net not in zone_nets and not net.connected}
    actual = {net for finding in pipeline.drc.findings
              if finding.code == "DRC-OPEN-NET" for net in finding.nets
              if net not in zone_nets}
    return tuple(sorted(reported | actual))


def _unaffected_copper(board: PhysicalBoard, changed: frozenset[str]):
    return (Counter(track for track in board.tracks if track.net not in changed),
            Counter(via for via in board.vias if via.net not in changed))


def _hard_drc_findings(board: PhysicalBoard) -> int:
    return sum(
        item.severity.value == "error"
        and item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}
        for item in run_physical_drc(board).findings
    )


def _candidate_placements(
    board: PhysicalBoard, pending: frozenset[PadReference],
    placement_options: PlacementPlannerOptions,
    feedback_options: EscapeFeedbackOptions,
):
    """Try alternate escape direction, then legal local moves and rotations."""

    yield board, None, "current placement / nearest escape"
    placements = {item.reference: item for item in board.placements}
    target_refs = sorted(
        {pad.component for pad in pending},
        key=lambda reference: (
            -sum(pad.component == reference for pad in pending), reference,
        ),
    )
    fixed = set(placement_options.fixed_references) | {
        rule.reference for rule in board.placement_rules
        if rule.fixed_position is not None
    }
    neighbor_refs = sorted(
        (reference for reference in placements
         if reference not in target_refs),
        key=lambda reference: (
            min(
                abs(placements[reference].position.x_nm
                    - placements[target].position.x_nm)
                + abs(placements[reference].position.y_nm
                      - placements[target].position.y_nm)
                for target in target_refs
            ), reference,
        ),
    )[:feedback_options.nearby_components]
    rules = {rule.reference: rule for rule in board.placement_rules}
    moved_candidates: list[tuple[PhysicalBoard, str]] = []
    for reference in (*target_refs, *neighbor_refs):
        if reference in fixed:
            continue
        current = placements[reference]
        rule = rules.get(reference)
        orientations = (
            rule.allowed_orientations if rule is not None
            else (Decimal(0), Decimal(90), Decimal(180), Decimal(270))
        )
        distance = feedback_options.movement_nm
        moves = [replace(current, position=Point(
            current.position.x_nm + dx * distance,
            current.position.y_nm + dy * distance,
        )) for dx, dy in (
            (1, 0), (-1, 0), (0, 1), (0, -1),
            (1, 1), (1, -1), (-1, 1), (-1, -1),
        )]
        moves.sort(key=lambda moved: (
            -min((
                (moved.position.x_nm - other.position.x_nm) ** 2
                + (moved.position.y_nm - other.position.y_nm) ** 2
                for other in board.placements if other.reference != reference
            ), default=1 << 60),
            moved.position.x_nm, moved.position.y_nm,
        ))
        changes = [*moves, *(
            replace(current, rotation_degrees=angle)
            for angle in orientations if angle != current.rotation_degrees
        )]
        for changed in changes:
            try:
                candidate = move_placement_unit(board, placements, reference, changed)
            except ValueError:
                continue
            if not placement_solution_is_legal(
                board, candidate, placement_options,
            ):
                continue
            updated = replace(board, placements=tuple(
                candidate[item.reference] for item in board.placements
            ))
            description = (
                f"{reference} rotate {changed.rotation_degrees} degrees"
                if changed.position == current.position else
                f"{reference} move to "
                f"({changed.position.x_nm}, {changed.position.y_nm}) nm"
            )
            moved_candidates.append((updated, description))
            yield updated, None, description
    yield board, "south", "current placement / south escape"
    for updated, description in moved_candidates:
        yield updated, "south", description + " / south escape"
    yield board, "north", "current placement / north escape"
    yield board, "east", "current placement / east escape"
    yield board, "west", "current placement / west escape"
