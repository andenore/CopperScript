"""Bounded transactional rerouting of a small net subset and its blocker cone.

The native filled-copper feedback (``filled_feedback``) reserves a bridge,
then reroutes the ordinary nets it displaced with the unified router. Every
retry starts afresh with all unrelated area copper restored; probe paths only
name blockers and are never accepted as geometry.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import replace
from hashlib import sha256
from math import hypot

from .critical import CriticalRoutingStatus
from .detailed import DetailedRoutingResult, DetailedRoutingStatus
from .drc import DrcDecision, PhysicalDrcPolicy, run_physical_drc
from .flow import PhysicalFlowStatus, RoutingPipelineResult
from .physical import PhysicalBoard, TrackSegment, Via
from .progress import ProgressCallback, emit
from .route_closure import close_detailed_lands
from .routing_clearance import RoutingClearanceIndex


def with_preserved_area(
    fixed: PhysicalBoard, tracks: tuple[TrackSegment, ...] | list[TrackSegment],
    vias: tuple[Via, ...] | list[Via], changed: frozenset[str],
) -> PhysicalBoard:
    """``fixed`` plus the given area copper of every net outside ``changed``."""
    return replace(fixed,
        tracks=(*fixed.tracks, *(track for track in tracks if track.net not in changed)),
        vias=(*fixed.vias, *(via for via in vias if via.net not in changed)))


def unaffected_copper(board: PhysicalBoard, changed: frozenset[str]):
    """Track and via multisets of every net outside ``changed``."""
    return (Counter(track for track in board.tracks if track.net not in changed),
            Counter(via for via in board.vias if via.net not in changed))


def reroute_local_dependencies(
    initial: RoutingPipelineResult, fixed: PhysicalBoard,
    movable_tracks: tuple[TrackSegment, ...], movable_vias: tuple[Via, ...],
    changed: frozenset[str], eligible: frozenset[str], *,
    maximum_local_blockers: int, maximum_dependency_expansions: int,
    on_progress: ProgressCallback | None = None,
) -> tuple[PhysicalBoard, DetailedRoutingResult | None, frozenset[str], int] | None:
    """Expand an exact blocker cone, never accepting relaxed probe geometry.

    The fixed prefix owns critical copper and accepted ground contacts.
    Placement is unchanged, so global evidence remains valid. Only completed
    ordinary nets may join the bounded cone (at most ``maximum_local_blockers``
    nets, ``maximum_dependency_expansions`` enlargements); failed probes fall
    back to the caller.
    """
    if not changed.issubset(eligible) or len(changed) > maximum_local_blockers:
        return None
    if not changed:
        return with_preserved_area(fixed, movable_tracks, movable_vias, changed), None, changed, 0
    from .unified_router import UnifiedRouterOptions, route_unified
    # A repair must not overwrite the main run's replay input artifact.
    settings = replace(initial.unified_options or UnifiedRouterOptions(), save_inputs_path=None)

    def search(source, names, kind, expansion):
        emit(on_progress, "zone_subset_search", "started", kind=kind,
             affected_nets=sorted(names), expansion=expansion, router="unified")
        result = route_unified(source, initial.placement_and_global.global_route,
            settings, fanout_accesses=None, required_accesses={},
            # Local transactions do not own/prune the fixed prefix.
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

    for expansion in range(maximum_dependency_expansions + 1):
        trial = with_preserved_area(fixed, movable_tracks, movable_vias, changed)
        actual = search(trial, changed, "transaction", expansion)
        if complete(actual, changed):
            # Even changed-net critical contacts remain owned by the prefix.
            if (Counter(fixed.tracks) - Counter(actual.board.tracks)
                    or Counter(fixed.vias) - Counter(actual.board.vias)
                    or unaffected_copper(actual.board, changed) != unaffected_copper(trial, changed)):
                return None
            return actual.board, actual, changed, expansion
        if expansion == maximum_dependency_expansions:
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
                or len(enlarged) > maximum_local_blockers):
            return None
        changed = enlarged
    return None


def merge_local_detail(
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
    locked = original.plane_stitch.board if original.plane_stitch else original.critical.board
    from .zone_geometry import distribution_zone_nets
    zone_nets = distribution_zone_nets(board)
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
