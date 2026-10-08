"""Conservative placement transactions retaining unrelated exact signal copper.

Global guides are recomputed, not relabelled. Critical endpoints and crowded
package owners are outside this first incremental scope. Ground contacts on
affected zone nets are rebuilt; their critical return vias remain locked.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, replace
from hashlib import sha256

from .boundary_access import analyze_boundary_access, reserve_boundary_access
from .critical import CriticalRoutingStatus, _fingerprint as critical_fingerprint
from .detailed import DetailedRouterOptions
from .drc import PhysicalDrcPolicy
from .escape_feedback import (EscapeFeedbackOptions, _failed_signals, _hard_drc_findings,
                             _merge_local_detail, _reroute_local_dependencies, _unaffected_copper)
from .fanout import FanoutOptions
from .flow import RoutingPipelineResult
from .physical import PhysicalBoard, RouteKind
from .placement import PlacementPlannerOptions, placement_solution_is_legal
from .plane import PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads
from .progress import ProgressCallback, emit
from .routeflow import FeedbackStatus
from .routing import GlobalRouterOptions, GlobalRoutingStatus, route_global
from .routing_clearance import RoutingClearanceIndex


@dataclass(frozen=True, slots=True)
class IncrementalPlacementResult:
    pipeline: RoutingPipelineResult
    plane_stitch: PlaneStitchResult
    repair_nets: tuple[str, ...]
    changed_references: tuple[str, ...]
    rebuilt_zone_nets: tuple[str, ...]
    dependency_expansions: int


def repair_placement_trial(
    initial: RoutingPipelineResult, trial: PhysicalBoard, baseline: PlaneStitchResult,
    early_options: PlaneStitchOptions, plane_options: PlaneStitchOptions,
    *, options: EscapeFeedbackOptions, detailed_options: DetailedRouterOptions,
    placement_options: PlacementPlannerOptions, global_options: GlobalRouterOptions | None = None,
    fanout_options: FanoutOptions | None = None, drc_policy: PhysicalDrcPolicy | None = None,
    on_progress: ProgressCallback | None = None,
) -> IncrementalPlacementResult | None:
    """Return an improving transaction, or None to request the full-pipeline fallback."""
    def fallback(reason: str) -> None:
        emit(on_progress, "zone_incremental_guard", "fallback", reason=reason)
        return None

    source = initial.placement_and_global.board
    poses = {pose.reference: pose for pose in initial.board.placements}
    if (trial.tracks or trial.vias or trial.zone_fills or baseline.board.zone_fills
            or replace(source, placements=trial.placements) != trial):
        return fallback("unsupported source change or filled geometry")
    if (len(trial.placements) != len(poses)
            or {pose.reference for pose in trial.placements} != set(poses)):
        return fallback("placement references changed")
    moved = frozenset(pose.reference for pose in trial.placements if pose != poses[pose.reference])
    if not moved:
        return fallback("placement unchanged")
    if (not initial.placement_and_global.full_route_certified
            or initial.critical.status is CriticalRoutingStatus.FAILED
            or initial.package_access is not None and not initial.package_access.ready):
        return fallback("initial routing/access gate failed")
    if not placement_solution_is_legal(source, {pose.reference: pose for pose in trial.placements}, placement_options):
        return fallback("illegal placement")
    cluster_refs = {member.reference for cluster in source.rigid_clusters for member in cluster.members}
    if moved & cluster_refs or any(pose.side != poses[pose.reference].side
                                   or pose.footprint != poses[pose.reference].footprint
                                   for pose in trial.placements):
        return fallback("rigid macro, footprint or side change")
    incident = {net.name for net in trial.nets if any(pad.component in moved for pad in net.pads)}
    protected = {rule.net for rule in source.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    protected.update(net for group in initial.critical.nets for net in group.nets)
    if incident & protected:
        return fallback("critical endpoint moved")
    if initial.fanout is not None:
        if initial.fanout.boundary_accesses:
            # Moving even an unrelated pad/keepout can invalidate the owned
            # collar path. Full preflight rebuilds paths and source evidence;
            # never let the old incremental prefix relabel/prune reservations.
            return fallback("owned boundary access requires full preflight")
        owned = {pad.component for pad in initial.fanout.accesses}
        owned.update(item.pad.component for item in initial.fanout.pin_analysis)
        owned.update(pad.component for pad in initial.fanout.pending_pads)
        minimum = (fanout_options or FanoutOptions()).minimum_component_pads
        if moved & owned or any(len(trial.footprints[pose.footprint].pads) >= minimum
                                for pose in trial.placements if pose.reference in moved):
            return fallback("crowded package access must be rebuilt")
    from .zone_geometry import distribution_zone_nets
    zone_nets = distribution_zone_nets(trial)
    reserved = early_options.only_pads or frozenset()
    rebuilt_zones = zone_nets & (incident | {net.name for net in trial.nets if any(pad in reserved for pad in net.pads)})
    eligible = frozenset(item.net for item in initial.detailed.nets if item.connected and item.net not in zone_nets)
    changed = frozenset(net.name for net in trial.nets if net.name in incident - zone_nets and len(net.pads) >= 2)
    if not changed.issubset(eligible) or len(changed) > options.maximum_local_blockers:
        return fallback("incident-net dependency limit or incomplete ordinary net")

    # Account for ordinary fanout vias pruned by the original detailed router.
    # Never silently lose actual critical copper or a return-stitch via.
    tracks, vias = Counter(baseline.board.tracks), Counter(baseline.board.vias)
    missing_tracks = Counter(initial.critical.locked_tracks) - tracks
    missing_vias = Counter(initial.critical.locked_vias) - vias
    fans = initial.fanout
    if (missing_tracks - Counter(fans.created_tracks if fans else ())
            or missing_vias - Counter(fans.created_vias if fans else ())):
        return fallback("critical ownership evidence missing")
    prefix_tracks = tuple((Counter(initial.critical.locked_tracks) & tracks).elements())
    prefix_vias = tuple((Counter(initial.critical.locked_vias) & vias).elements())
    rest_tracks = tuple((tracks - Counter(prefix_tracks)).elements())
    rest_vias = tuple((vias - Counter(prefix_vias)).elements())
    movable_tracks = tuple(item for item in rest_tracks if item.net not in zone_nets)
    movable_vias = tuple(item for item in rest_vias if item.net not in zone_nets)
    retained_tracks = tuple(item for item in rest_tracks if item.net in zone_nets - rebuilt_zones)
    retained_vias = tuple(item for item in rest_vias if item.net in zone_nets - rebuilt_zones)
    prefix = replace(trial, tracks=prefix_tracks, vias=prefix_vias, metadata=initial.critical.board.metadata)
    if _hard_drc_findings(prefix):
        return fallback("moved pad/keepout collides with locked copper")
    early = stitch_zone_pads(replace(prefix, tracks=(*prefix_tracks, *retained_tracks),
                                     vias=(*prefix_vias, *retained_vias)), early_options)
    if early.pending_pads or _hard_drc_findings(early.board):
        return fallback("early ground/access reservation failed")
    clearance = RoutingClearanceIndex(early.board)
    collisions = {track.net for track in movable_tracks if not clearance.can_track(
        track.net, track.start, track.end, track.width_nm, track.layer)}
    collisions.update(via.net for via in movable_vias if not clearance.can_via(
        via.net, via.position, via.size_nm, via.from_layer, via.to_layer, via.drill_nm,
        check_hole_copper=via.finish == "filled-capped"))
    changed |= frozenset(collisions)
    if not changed.issubset(eligible) or len(changed) > options.maximum_local_blockers:
        return fallback("moved geometry/escape dependency limit")

    emit(on_progress, "zone_moved_global", "started", changed_references=sorted(moved))
    global_route = route_global(trial, global_options)
    emit(on_progress, "zone_moved_global", "finished", status=global_route.status.value)
    if global_route.status is not GlobalRoutingStatus.SUCCESS:
        return fallback("fresh global routing failed")
    metadata = {**early.board.metadata, "global_routing_fingerprint": global_route.routing_fingerprint}
    prefix = replace(prefix, metadata=metadata)
    early = replace(early, board=replace(early.board, metadata=metadata))
    critical = replace(initial.critical, board=prefix, locked_tracks=prefix_tracks, locked_vias=prefix_vias,
        global_routing_fingerprint=global_route.routing_fingerprint,
        routing_fingerprint=critical_fingerprint(global_route.routing_fingerprint, list(prefix_tracks),
                                                 list(prefix_vias), list(initial.critical.nets)))
    access_fanout = None
    if fans is not None:
        fan_tracks = tuple((Counter(fans.created_tracks) & Counter(prefix_tracks)).elements())
        fan_vias = tuple((Counter(fans.created_vias) & Counter(prefix_vias)).elements())
        access_fanout = replace(fans, board=replace(trial, tracks=fan_tracks, vias=fan_vias,
            metadata={**trial.metadata, "global_routing_fingerprint": global_route.routing_fingerprint}),
            created_tracks=fan_tracks, created_vias=fan_vias, added_track_count=len(fan_tracks),
            added_via_count=len(fan_vias), pin_analysis=(), assignment=None)
        critical = replace(critical, reserved_track_count=len(fan_tracks), reserved_via_count=len(fan_vias))
    access = (replace(initial.package_access, source=trial, global_route=global_route,
        fanout=access_fanout, critical=critical, plane_stitch=early,
        hard_findings=0, trials=(), accepted_moves=0, pattern_trials=(), boundary=None)
        if initial.package_access is not None else None)
    if access is not None:
        # Even moving a sparse neighbour can close an ordinary collar channel.
        # Never relabel or retain old witness coordinates after placement.
        if access_fanout is None or initial.package_access.boundary is None:
            return fallback("boundary ownership evidence missing")
        try:
            boundary = analyze_boundary_access(early.board, access_fanout,
                                               initial.package_access.boundary.options)
        except ValueError:
            return fallback("boundary source requires full preflight")
        if not boundary.ready:
            return fallback("placement closes package boundary access")
        access = replace(access, boundary=boundary)
        access_fanout = reserve_boundary_access(early.board, access_fanout, boundary,
            deferred_nets=frozenset(distribution_zone_nets(early.board))
            if detailed_options.defer_zone_nets else frozenset())
        early = replace(early, board=access_fanout.board)
    boot = replace(initial,
        placement_and_global=replace(initial.placement_and_global, board=trial, global_route=global_route,
            full_route_certified=True, status=FeedbackStatus.PASS, iterations=(),
            placement_candidate=initial.placement_and_global.placement_candidate + "-incremental"),
        critical=critical, fanout=replace(access_fanout, board=early.board) if access_fanout else None,
        plane_stitch=early, package_access=access, critical_feedback=None,
        detailed=replace(initial.detailed, global_routing_fingerprint=global_route.routing_fingerprint,
            routing_fingerprint=sha256(global_route.routing_fingerprint.encode()).hexdigest()))
    repaired = _reroute_local_dependencies(boot, early.board, movable_tracks, movable_vias,
        changed, eligible, detailed_options, options, on_progress=on_progress)
    if repaired is None:
        return fallback("bounded subset repair failed")
    board, reroute, changed, expansions = repaired
    merged = _merge_local_detail(boot, board, reroute, changed, drc_policy=drc_policy)
    late = stitch_zone_pads(merged.board, plane_options)
    if (_unaffected_copper(merged.board, changed | frozenset(rebuilt_zones))
            != _unaffected_copper(baseline.board, changed | frozenset(rebuilt_zones))
            or _hard_drc_findings(late.board) > _hard_drc_findings(baseline.board)
            or not set(_failed_signals(merged, zone_nets)).issubset(_failed_signals(initial, zone_nets))
            or len(late.pending_pads) >= len(baseline.pending_pads)):
        return fallback("whole-board closure/geometry did not improve safely")
    return IncrementalPlacementResult(merged, late, tuple(sorted(changed)), tuple(sorted(moved)),
                                      tuple(sorted(rebuilt_zones)), expansions)
