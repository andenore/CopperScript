"""Transactional package-escape and placement feedback for zone-net pads.

Only a small set of late-failing pads is reserved early. Each candidate starts
from an unrouted placement; no component is moved beneath accepted copper.
The ordinary routes and late plane contacts are then rebuilt and compared.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from decimal import Decimal
from collections import Counter
from hashlib import sha256

from .critical import CriticalRoutingStatus
from .detailed import DetailedRouterOptions, DetailedRoutingResult, route_detailed
from .drc import PhysicalDrcPolicy, run_physical_drc
from .fanout import FanoutOptions
from .flow import RoutingPipelineResult, run_routing_pipeline
from .physical import PadReference, PhysicalBoard, Placement, Point, nm_from_mm
from .placement import PlacementPlannerOptions, placement_solution_is_legal
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .routeflow import PlacementRoutingFeedbackOptions
from .routing import GlobalRouterOptions, GlobalRoutingStatus
from .routing_clearance import RoutingClearanceIndex


@dataclass(frozen=True, slots=True)
class EscapeFeedbackOptions:
    maximum_trials: int = 4
    maximum_local_trials: int = 6
    maximum_local_blockers: int = 4
    movement_nm: int = nm_from_mm("0.5")
    nearby_components: int = 3

    def __post_init__(self) -> None:
        if (self.maximum_trials < 0 or self.maximum_local_trials < 0
                or not 1 <= self.maximum_local_blockers <= 8
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
    options: EscapeFeedbackOptions | None = None,
    _reserved_pads: frozenset[PadReference] = frozenset(),
) -> EscapeFeedbackResult:
    """Repair late pad escapes without sacrificing completed signal routes."""

    options = options or EscapeFeedbackOptions()
    placement_options = placement_options or PlacementPlannerOptions()
    baseline_stitch = stitch_zone_pads(initial.board, plane_options)
    if not baseline_stitch.pending_pads:
        return EscapeFeedbackResult(initial, baseline_stitch, ())
    if options.maximum_local_trials:
        local = _repair_with_local_ripup(
            initial, baseline_stitch, plane_options,
            detailed_options or DetailedRouterOptions(), options,
        )
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
                options=replace(options, maximum_local_trials=0),
                _reserved_pads=_reserved_pads,
            )
            return EscapeFeedbackResult(
                continuation.pipeline, continuation.plane_stitch,
                (*local.attempts, *continuation.attempts),
            )
    if options.maximum_trials == 0:
        return EscapeFeedbackResult(initial, baseline_stitch, ())
    targets = frozenset(baseline_stitch.pending_pads)
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
            plane_options, only_pads=targets | _reserved_pads,
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
            plane_stitch_options=early_options,
        )
        routed_trials += 1
        late = stitch_zone_pads(trial_pipeline.board, plane_options)
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
        ))
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
                    options=replace(
                        options,
                        maximum_trials=options.maximum_trials-routed_trials,
                    ),
                    _reserved_pads=(
                        _reserved_pads | (targets - pending_early)
                    ),
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


def _repair_with_local_ripup(
    initial: RoutingPipelineResult,
    baseline: PlaneStitchResult,
    plane_options: PlaneStitchOptions,
    detailed_options: DetailedRouterOptions,
    options: EscapeFeedbackOptions,
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
            trial_board = replace(
                candidate.board,
                tracks=tuple((*candidate.board.tracks,
                              *(track for track in movable_tracks
                                if track.net not in displaced))),
                vias=tuple((*candidate.board.vias,
                            *(via for via in movable_vias
                              if via.net not in displaced))),
            )
            reroute = (
                route_detailed(
                    trial_board,
                    initial.placement_and_global.global_route,
                    detailed_options,
                    fanout_accesses=(initial.fanout.accesses
                                    if initial.fanout else None),
                    only_nets=displaced,
                ) if displaced else None
            )
            if reroute is not None and any(not item.connected for item in reroute.nets):
                continue
            routed_board = reroute.board if reroute is not None else trial_board
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
            )
            return EscapeFeedbackResult(merged, late, (attempt,))
    return None


def _merge_local_detail(
    original: RoutingPipelineResult,
    board: PhysicalBoard,
    reroute: DetailedRoutingResult | None,
    changed: frozenset[str],
) -> RoutingPipelineResult:
    """Preserve full-board metrics while replacing a bounded net subset."""

    old = {item.net: item for item in original.detailed.nets}
    new = {item.net: item for item in reroute.nets} if reroute is not None else {}
    nets = tuple(new.get(item.net, item) for item in original.detailed.nets)
    metrics = original.detailed.metrics
    old_items = [old[name] for name in changed]
    new_items = [new[name] for name in changed]
    metrics = replace(
        metrics,
        track_count=metrics.track_count - sum(item.track_count for item in old_items)
                    + sum(item.track_count for item in new_items),
        via_count=metrics.via_count - sum(item.via_count for item in old_items)
                  + sum(item.via_count for item in new_items),
        total_length_nm=metrics.total_length_nm
                        - sum(item.length_nm for item in old_items)
                        + sum(item.length_nm for item in new_items),
    )
    fingerprint = sha256(repr((
        original.detailed.routing_fingerprint, board.tracks,
        board.vias,
    )).encode()).hexdigest()
    detailed = replace(
        original.detailed, board=board, nets=nets, metrics=metrics,
        routing_fingerprint=fingerprint,
    )
    return replace(
        original, detailed=detailed,
        drc=run_physical_drc(board),
    )


def _failed_signals(
    pipeline: RoutingPipelineResult, zone_nets: set[str],
) -> tuple[str, ...]:
    return tuple(
        net.net for net in pipeline.detailed.nets
        if net.net not in zone_nets and not net.connected
    )


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
            candidate = {**placements, reference: changed}
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
