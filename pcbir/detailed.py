"""Guide-aware deterministic general detailed router."""

from __future__ import annotations

from bisect import bisect_left
from dataclasses import asdict, dataclass, field, replace
from enum import Enum
from hashlib import sha256
from heapq import heappop, heappush
from itertools import permutations
import json
from math import hypot, isqrt
from types import MappingProxyType
from typing import Iterable, Mapping

from .geometry import (RoundedConvexShape, point_on_segment, point_in_polygon,
                       segment_in_polygon, shape_distance_squared)
from .physical import (
    BoardSide,
    CopperLayer,
    CopperKeepout,
    NetRoutingRule,
    PadKind,
    PadReference,
    PhysicalBoard,
    PhysicalNet,
    Placement,
    Point,
    RouteKind,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .placement import (
    resolved_copper_keepouts,
    transformed_pad_position,
)
from .routing import GlobalNetRoute, GlobalRoutingResult
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_vias import physical_via_span
from .routing_costs import COST_UNIT, length_cost, preference_cost
from .routing_guides import GuideExposure, guide_transition_cost
from .surface_path import _track_inside_board


class DetailedRoutingStatus(str, Enum):
    SUCCESS = "success"
    PARTIAL = "partial"


@dataclass(frozen=True, slots=True, order=True)
class DetailedNode:
    layer_index: int
    x_index: int
    y_index: int


@dataclass(frozen=True, slots=True)
class DetailedSearchPolicy:
    """Search-owner telemetry, not geometry, signoff or final closure metrics.

    Attach to individual net results so accepted subset repairs can retain a
    mixture of policies without claiming one effective policy for the board.
    Failure/overflow pairs are measured before physical-land/native closure.
    """

    requested_layer_preference_cost: int
    requested_direction_preference_cost: int
    effective_layer_preference_cost: int
    effective_direction_preference_cost: int
    neutral_fallback_attempted: bool
    neutral_fallback_selected: bool
    scored_nets: tuple[str, ...]
    preferred_failure_overflow: tuple[int, int]
    neutral_failure_overflow: tuple[int, int] | None = None


@dataclass(frozen=True, slots=True)
class DetailedNetResult:
    net: str
    connected: bool
    track_count: int
    via_count: int
    length_nm: int
    guide_deviation_count: int
    diagnostics: tuple[str, ...] = ()
    orthogonal_mode_used: bool = False
    search_policy: DetailedSearchPolicy | None = None


@dataclass(frozen=True, slots=True)
class DetailedRoutingMetrics:
    routed_net_count: int
    unrouted_net_count: int
    conflict_resource_count: int
    total_conflict_overflow: int
    track_count: int
    via_count: int
    total_length_nm: int
    passes: int

    @property
    def quality_vector(self) -> tuple[int, ...]:
        return (
            self.unrouted_net_count,
            self.total_conflict_overflow,
            self.conflict_resource_count,
            self.via_count,
            self.total_length_nm,
        )


@dataclass(frozen=True, slots=True)
class DetailedRoutingResult:
    status: DetailedRoutingStatus
    board: PhysicalBoard
    nets: tuple[DetailedNetResult, ...]
    metrics: DetailedRoutingMetrics
    locked_track_count: int
    locked_via_count: int
    global_routing_fingerprint: str
    routing_fingerprint: str

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-detailed-route/v0.1",
            "status": self.status.value,
            "global_routing_fingerprint": self.global_routing_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "locked_track_count": self.locked_track_count,
            "locked_via_count": self.locked_via_count,
            "metrics": {
                "routed_net_count": self.metrics.routed_net_count,
                "unrouted_net_count": self.metrics.unrouted_net_count,
                "conflict_resource_count": self.metrics.conflict_resource_count,
                "total_conflict_overflow": self.metrics.total_conflict_overflow,
                "track_count": self.metrics.track_count,
                "via_count": self.metrics.via_count,
                "total_length_nm": self.metrics.total_length_nm,
                "passes": self.metrics.passes,
            },
            "nets": [
                {
                    "net": item.net,
                    "connected": item.connected,
                    "track_count": item.track_count,
                    "via_count": item.via_count,
                    "length_nm": item.length_nm,
                    "guide_deviation_count": item.guide_deviation_count,
                    "diagnostics": list(item.diagnostics),
                    "orthogonal_mode_used": item.orthogonal_mode_used,
                    "search_policy": asdict(item.search_policy) if item.search_policy else None,
                }
                for item in self.nets
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True, slots=True)
class DetailedRouterOptions:
    pitch_nm: int = nm_from_mm("0.5")
    maximum_passes: int = 10
    present_penalty: int = 100
    history_penalty: int = 30
    via_cost: int = 80
    bend_cost: int = 12
    layer_preference_cost: int = 4
    direction_preference_cost: int = 2
    guide_margin_nm: int = nm_from_mm("1")
    allow_guide_deviation: bool = True
    any_angle_cleanup: bool = True
    octilinear_search: bool = True
    orthogonal_first_min_pads: int = 16
    pin_access_candidates: int = 8
    maximum_search_states: int = 50_000
    heuristic_weight_percent: int = 100
    enable_soft_ripup: bool = False
    constrained_pins_first: bool = False
    progressive_guides: bool = False
    repair_budget_multiplier: int = 1
    defer_zone_nets: bool = True
    maximum_ripup_blockers: int = 4

    def __post_init__(self) -> None:
        if min(
            self.pitch_nm, self.maximum_passes, self.pin_access_candidates,
            self.maximum_search_states,
        ) <= 0:
            raise ValueError("detailed router pitch and passes must be positive")
        if min(
            self.present_penalty,
            self.history_penalty,
            self.via_cost,
            self.bend_cost,
            self.layer_preference_cost,
            self.direction_preference_cost,
            self.guide_margin_nm,
        ) < 0:
            raise ValueError("detailed router costs cannot be negative")
        if not 100 <= self.heuristic_weight_percent <= 300:
            raise ValueError("detailed router heuristic weight must be 100..300 percent")
        if not 1 <= self.repair_budget_multiplier <= 10:
            raise ValueError("detailed router repair budget multiplier must be 1..10")
        if not 1 <= self.maximum_ripup_blockers <= 8:
            raise ValueError("maximum rip-up blockers must be 1..8")
        if self.orthogonal_first_min_pads < 3:
            raise ValueError("orthogonal-first threshold must be at least 3 pads")


@dataclass(frozen=True, slots=True)
class _Grid:
    layers: tuple[CopperLayer, ...]
    xs: tuple[int, ...]
    ys: tuple[int, ...]
    board: PhysicalBoard
    blocked: frozenset[DetailedNode]
    pitch_nm: int
    diagonal_successors: dict[tuple[tuple[int, ...], tuple[int, ...], int, int], tuple[tuple[int, int], ...]] = field(
        default_factory=dict, compare=False, repr=False,
    )
    obstacle_cache: dict[int, tuple[PhysicalBoard, tuple[CopperKeepout, ...]]] = field(
        default_factory=dict, compare=False, repr=False,
    )
    line_clear_cache: dict[tuple[object, ...], bool] = field(
        default_factory=dict, compare=False, repr=False,
    )

    def point(self, node: DetailedNode) -> Point:
        return Point(self.xs[node.x_index], self.ys[node.y_index])


@dataclass(frozen=True, slots=True)
class _NetAttempt:
    result: DetailedNetResult
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...]
    resources: frozenset[str]


@dataclass(frozen=True, slots=True)
class _Pass:
    nets: tuple[_NetAttempt, ...]
    usage: Mapping[str, int]
    metrics: DetailedRoutingMetrics


class _SearchBudgetExceeded(Exception):
    """A bounded detailed search exhausted its configured state budget."""


def route_detailed(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    options: DetailedRouterOptions | None = None,
    *,
    fanout_accesses: Mapping[PadReference, Point] | None = None,
    fanout_created_vias: frozenset[tuple[str, Point]] | None = None,
    only_nets: frozenset[str] | None = None,
) -> DetailedRoutingResult:
    """Route ordinary nets, optionally a repair subset, preserving locked copper."""

    options = options or DetailedRouterOptions()
    fanout_accesses = fanout_accesses or {}
    rules = {item.net: item for item in board.net_routing_rules}
    guides = {item.net: item for item in global_route.routes}
    zone_nets = {zone.net for zone in board.zones} if options.defer_zone_nets else set()
    if only_nets is not None:
        unknown = only_nets - {net.name for net in board.nets}
        if unknown:
            raise ValueError(f"unknown repair nets: {', '.join(sorted(unknown))}")
        # A bounded repair owns neither the fanout nor the vias of other nets.
        selected_pads = {pad for net in board.nets if net.name in only_nets
                         for pad in net.pads}
        fanout_accesses = {pad: anchor for pad, anchor in fanout_accesses.items()
                           if pad in selected_pads}
        if fanout_created_vias is None:
            # Without ownership evidence, locked input vias are not disposable.
            fanout_created_vias = frozenset()
    deferred = [
        net for net in board.nets if len(net.pads) >= 2 and net.name in zone_nets
        and (only_nets is None or net.name in only_nets)
    ]
    general_nets = [
        net
        for net in board.nets
        if len(net.pads) >= 2
        and (only_nets is None or net.name in only_nets)
        and net.name not in zone_nets
        and (rules.get(net.name) is None or rules[net.name].kind is RouteKind.GENERAL)
    ]
    history: dict[str, int] = {}
    best: _Pass | None = None
    completed: list[_Pass] = []
    completed_passes = 0
    for pass_index in range(1, options.maximum_passes + 1):
        completed_passes = pass_index
        pass_options = replace(
            options,
            heuristic_weight_percent=_pass_heuristic_weight(options, pass_index),
        )
        clearance = RoutingClearanceIndex(board)
        usage: dict[str, int] = {}
        attempts: list[_NetAttempt] = [
            _failed(net.name, "zone net awaits verified fill and pad stitching")
            for net in deferred
        ]
        ordered_nets = sorted(
            general_nets,
            key=lambda item: (-len(item.pads), _net_span(board, item.pads), item.name),
        )
        if pass_index == 2:
            ordered_nets.reverse()
        elif pass_index >= 3 and best is not None:
            failed = {item.result.net for item in best.nets if not item.result.connected}
            if pass_index <= 4:
                ordered_nets.sort(key=lambda item: (
                    item.name not in failed,
                    -len(item.pads) if pass_index % 2 else len(item.pads),
                    _net_span(board, item.pads), item.name,
                ))
            else:
                # Repeating the same two sorts cannot discover a new legal
                # route order once the best pass stagnates. Diversify later
                # passes reproducibly while keeping failed nets prioritized.
                ordered_nets.sort(key=lambda item: (
                    item.name not in failed,
                    sha256(f"{pass_index}:{item.name}".encode()).digest(),
                ))
        for net in ordered_nets:
            grid = _build_grid(board, pass_options, net.pads, fanout_accesses)
            guide = guides.get(net.name)
            attempt = _route_net(
                board,
                grid,
                net.name,
                net.pads,
                rules.get(net.name),
                guide,
                usage,
                history,
                clearance,
                pass_options,
                fanout_accesses=fanout_accesses,
            )
            attempts.append(attempt)
            if attempt.result.connected:
                for resource in attempt.resources:
                    usage[resource] = usage.get(resource, 0) + 1
                for track in attempt.tracks:
                    clearance.add_track(track)
                for via in attempt.vias:
                    clearance.add_via(via)
        overflow = [value - 1 for value in usage.values() if value > 1]
        metrics = DetailedRoutingMetrics(
            routed_net_count=sum(item.result.connected for item in attempts),
            unrouted_net_count=sum(not item.result.connected for item in attempts),
            conflict_resource_count=len(overflow),
            total_conflict_overflow=sum(overflow),
            track_count=sum(len(item.tracks) for item in attempts if item.result.connected),
            via_count=sum(len(item.vias) for item in attempts if item.result.connected),
            total_length_nm=sum(item.result.length_nm for item in attempts if item.result.connected),
            passes=pass_index,
        )
        current = _Pass(tuple(attempts), usage, metrics)
        completed.append(current)
        if best is None or current.metrics.quality_vector < best.metrics.quality_vector:
            best = current
        if metrics.unrouted_net_count == 0 and metrics.total_conflict_overflow == 0:
            best = current
            break
        for resource, value in usage.items():
            if value > 1:
                history[resource] = history.get(resource, 0) + value - 1
    assert best is not None
    if len(completed) > 1 and best.metrics.unrouted_net_count:
        repaired = _repair_from_passes(
            board, general_nets, rules, guides, best, completed, options,
            fanout_accesses,
        )
        if repaired.metrics.quality_vector < best.metrics.quality_vector:
            best = repaired
    # Layer/direction preferences must not strand a signal merely because
    # they changed search ordering within a finite state budget. Compare a
    # neutral-cost reroute only when an ordinary net was left open; zone nets
    # deliberately deferred to fill are not a search failure.
    neutral_metrics = None
    if ((options.layer_preference_cost or options.direction_preference_cost)
            and any(not item.result.connected and item.result.net not in zone_nets
                    for item in best.nets)):
        neutral = route_detailed(
            board, global_route,
            replace(options, layer_preference_cost=0,
                    direction_preference_cost=0),
            fanout_accesses=fanout_accesses,
            fanout_created_vias=fanout_created_vias,
            only_nets=only_nets,
        )
        neutral_metrics = neutral.metrics
        if (neutral.metrics.unrouted_net_count,
                neutral.metrics.total_conflict_overflow) < (
                best.metrics.unrouted_net_count,
                best.metrics.total_conflict_overflow):
            return replace(neutral, nets=_with_search_policy(
                neutral.nets, zone_nets, options, best.metrics, neutral_metrics, True,
            ))
    metrics = replace(best.metrics, passes=completed_passes)
    new_tracks = tuple(track for item in best.nets if item.result.connected for track in item.tracks)
    new_vias = tuple(via for item in best.nets if item.result.connected for via in item.vias)
    success = metrics.unrouted_net_count == 0 and metrics.total_conflict_overflow == 0
    metadata = dict(board.metadata)
    metadata.update(
        {
            "detailed_routing": "complete" if success else "partial",
            "global_routing_fingerprint": global_route.routing_fingerprint,
            "fabrication_ready": "false",
        }
    )
    all_tracks = tuple((*board.tracks, *new_tracks))
    all_vias = tuple((*board.vias, *new_vias))
    if fanout_accesses:
        cleanup_accesses = fanout_accesses
        if only_nets is not None:
            # A failed repair must not remove the input net's locked escape.
            successful_pads = {pad for net in board.nets
                               if any(item.result.net == net.name and item.result.connected
                                      for item in best.nets) for pad in net.pads}
            cleanup_accesses = {pad: anchor for pad, anchor in fanout_accesses.items()
                                if pad in successful_pads}
        all_tracks, all_vias = _prune_fanout_copper(
            board, all_tracks, all_vias, cleanup_accesses,
            frozenset(item.result.net for item in best.nets if item.result.connected),
            fanout_created_vias,
        )
    routed = replace(
        board,
        tracks=all_tracks,
        vias=all_vias,
        metadata=MappingProxyType(metadata),
    )
    fingerprint = _fingerprint(global_route.routing_fingerprint, routed, metrics)
    return DetailedRoutingResult(
        DetailedRoutingStatus.SUCCESS if success else DetailedRoutingStatus.PARTIAL,
        routed,
        _with_search_policy(tuple(item.result for item in best.nets), zone_nets,
                            options, best.metrics, neutral_metrics, False),
        metrics,
        len(board.tracks),
        len(board.vias),
        global_route.routing_fingerprint,
        fingerprint,
    )


def _with_search_policy(
    nets: tuple[DetailedNetResult, ...], zone_nets: set[str],
    options: DetailedRouterOptions, preferred: DetailedRoutingMetrics,
    neutral: DetailedRoutingMetrics | None, selected: bool,
) -> tuple[DetailedNetResult, ...]:
    policy = DetailedSearchPolicy(
        options.layer_preference_cost, options.direction_preference_cost,
        0 if selected else options.layer_preference_cost,
        0 if selected else options.direction_preference_cost,
        neutral is not None, selected,
        tuple(sorted(item.net for item in nets)),
        (preferred.unrouted_net_count, preferred.total_conflict_overflow),
        (neutral.unrouted_net_count, neutral.total_conflict_overflow) if neutral else None,
    )
    return tuple(replace(item, search_policy=policy) if item.net not in zone_nets
                 else replace(item, search_policy=None) for item in nets)


def _pass_heuristic_weight(options: DetailedRouterOptions, pass_index: int) -> int:
    """Diversify later bounded passes without sacrificing the baseline passes."""

    if pass_index <= 2:
        return options.heuristic_weight_percent
    return min(300, options.heuristic_weight_percent + 50 * (1 + (pass_index - 3) % 4))


def _prune_fanout_copper(
    board: PhysicalBoard,
    tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...],
    accesses: Mapping[PadReference, Point],
    successful_nets: frozenset[str],
    created_vias: frozenset[tuple[str, Point]] | None,
) -> tuple[tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Remove abandoned stubs and vias unused on a second copper layer."""

    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads}
    placement_by_ref = {item.reference: item for item in board.placements}
    abandoned: set[tuple[str, Point, Point]] = set()
    anchors: set[tuple[str, Point]] = set()
    for pad, anchor in accesses.items():
        net = net_by_pad[pad]
        anchors.add((net, anchor))
        if net not in successful_nets:
            original = transformed_pad_position(
                board, placement_by_ref[pad.component], pad.pad,
            )
            abandoned.add((net, original, anchor))
            abandoned.add((net, anchor, original))
    tracks = tuple(track for track in tracks
                   if (track.net, track.start, track.end) not in abandoned)
    used_layers: dict[tuple[str, Point], set[CopperLayer]] = {
        anchor: set() for anchor in anchors
    }
    for track in tracks:
        for key in used_layers:
            net, point = key
            if track.net == net and point_on_segment(point, track.start, track.end):
                used_layers[key].add(track.layer)
    prunable = anchors if created_vias is None else anchors & created_vias
    vias = tuple(via for via in vias
                 if (via.net, via.position) not in prunable
                 or len(used_layers[(via.net, via.position)]) >= 2)
    return tracks, vias


def _repair_from_passes(
    board: PhysicalBoard,
    nets: list[PhysicalNet],
    rules: Mapping[str, NetRoutingRule],
    guides: Mapping[str, GlobalNetRoute],
    best: _Pass,
    completed: list[_Pass],
    options: DetailedRouterOptions,
    fanout_accesses: Mapping[PadReference, Point] | None = None,
) -> _Pass:
    """Keep the best legal pass and add compatible routes found in other passes."""

    selected = {item.result.net: item for item in best.nets}
    fanout_accesses = fanout_accesses or {}
    clearance = RoutingClearanceIndex(board)
    usage = dict(best.usage)
    for item in best.nets:
        if item.result.connected:
            for track in item.tracks:
                clearance.add_track(track)
            for via in item.vias:
                clearance.add_via(via)

    alternatives: dict[str, list[_NetAttempt]] = {}
    for completed_pass in completed:
        for item in completed_pass.nets:
            if item.result.connected and not selected[item.result.net].result.connected:
                alternatives.setdefault(item.result.net, []).append(item)

    for net in nets:
        if selected[net.name].result.connected:
            continue
        candidates = sorted(
            alternatives.get(net.name, ()),
            key=lambda item: (item.result.via_count, item.result.length_nm),
        )
        if options.enable_soft_ripup:
            soft_candidate = _route_net(
                board, _build_grid(board, options, net.pads, fanout_accesses), net.name, net.pads,
                rules.get(net.name), guides.get(net.name), usage, {}, clearance,
                options, allow_movable_conflicts=True, fanout_accesses=fanout_accesses,
            )
            if soft_candidate.result.connected:
                candidates.append(soft_candidate)
        for candidate in candidates:
            if not all(
                clearance.can_track(
                    net.name, track.start, track.end, track.width_nm, track.layer,
                ) for track in candidate.tracks
            ) or not all(
                clearance.can_via(
                    net.name, via.position, via.size_nm, via.from_layer, via.to_layer,
                ) for via in candidate.vias
            ):
                continue
            selected[net.name] = candidate
            for resource in candidate.resources:
                usage[resource] = usage.get(resource, 0) + 1
            for track in candidate.tracks:
                clearance.add_track(track)
            for via in candidate.vias:
                clearance.add_via(via)
            break

    # A candidate from another pass may be blocked by several
    # already-routed nets. Rip those nets up transactionally, install the
    # candidate, then retain the swap only if every evicted net reroutes.
    net_by_name = {net.name: net for net in nets}
    for net in nets:
        if selected[net.name].result.connected:
            continue
        candidates = sorted(
            alternatives.get(net.name, ()),
            key=lambda item: (item.result.via_count, item.result.length_nm),
        )
        if options.enable_soft_ripup:
            soft_candidate = _route_net(
                board, _build_grid(board, options, net.pads, fanout_accesses), net.name, net.pads,
                rules.get(net.name), guides.get(net.name), usage, {}, clearance,
                options, allow_movable_conflicts=True, fanout_accesses=fanout_accesses,
            )
            if soft_candidate.result.connected:
                candidates.append(soft_candidate)
        for candidate in candidates:
            blockers: set[str] = set()
            immutable = False
            for track in candidate.tracks:
                names, locked = clearance.blocking_track_nets(track)
                blockers.update(names)
                immutable |= locked
            for via in candidate.vias:
                names, locked = clearance.blocking_via_nets(via)
                blockers.update(names)
                immutable |= locked
            if immutable or not 1 <= len(blockers) <= options.maximum_ripup_blockers:
                continue
            if any(
                name not in net_by_name or not selected[name].result.connected
                for name in blockers
            ):
                continue
            trial = {**selected, net.name: candidate}
            trial_clearance = RoutingClearanceIndex(board)
            for name, item in trial.items():
                if not item.result.connected or name in blockers or name == net.name:
                    continue
                for track in item.tracks:
                    trial_clearance.add_track(track)
                for via in item.vias:
                    trial_clearance.add_via(via)
            if not all(
                trial_clearance.can_track(
                    net.name, track.start, track.end, track.width_nm, track.layer,
                ) for track in candidate.tracks
            ) or not all(
                trial_clearance.can_via(
                    net.name, via.position, via.size_nm, via.from_layer, via.to_layer,
                ) for via in candidate.vias
            ):
                continue
            for track in candidate.tracks:
                trial_clearance.add_track(track)
            for via in candidate.vias:
                trial_clearance.add_via(via)
            rerouted: dict[str, _NetAttempt] = {}
            # Order is a first-class repair variable. Rebuild the index for
            # every trial so failed partial reroutes cannot leak copper.
            for order in _ripup_orders(blockers):
                trial_clearance = RoutingClearanceIndex(board)
                for name, item in trial.items():
                    if not item.result.connected or name in blockers or name == net.name:
                        continue
                    for track in item.tracks:
                        trial_clearance.add_track(track)
                    for via in item.vias:
                        trial_clearance.add_via(via)
                for track in candidate.tracks:
                    trial_clearance.add_track(track)
                for via in candidate.vias:
                    trial_clearance.add_via(via)
                rerouted = {}
                for blocker_name in order:
                    blocker = net_by_name[blocker_name]
                    attempt = _route_net(
                        board, _build_grid(board, options, blocker.pads, fanout_accesses),
                        blocker.name, blocker.pads, rules.get(blocker.name),
                        guides.get(blocker.name), {}, {}, trial_clearance, options,
                        fanout_accesses=fanout_accesses,
                    )
                    if not attempt.result.connected:
                        break
                    rerouted[blocker_name] = attempt
                    for track in attempt.tracks:
                        trial_clearance.add_track(track)
                    for via in attempt.vias:
                        trial_clearance.add_via(via)
                if len(rerouted) == len(blockers):
                    break
            if len(rerouted) != len(blockers):
                continue
            selected.update(rerouted)
            selected[net.name] = candidate
            clearance = trial_clearance
            usage = _attempt_usage(selected.values())
            break
    # The merged geometry may open a different corridor for a net that no
    # complete pass could route. Retry it against the exact merged copper.
    for net in nets:
        if selected[net.name].result.connected:
            continue
        repair_options = replace(
            options,
            maximum_search_states=(
                options.maximum_search_states * options.repair_budget_multiplier
            ),
        )
        attempt = _route_net(
            board, _build_grid(board, repair_options, net.pads, fanout_accesses), net.name, net.pads,
            rules.get(net.name), guides.get(net.name), usage, {}, clearance,
            repair_options, fanout_accesses=fanout_accesses,
        )
        # A coarse global grid can be topologically disconnected around fine
        # pitch pads even when exact copper clearance permits a route. Refine
        # only a proven no-path search; a budget-exhausted search needs more
        # states, not a larger graph.
        if (not attempt.result.connected
                and any("cannot reach" in message for message in attempt.result.diagnostics)
                and repair_options.pitch_nm > nm_from_mm("0.25")):
            refined_options = replace(
                repair_options,
                pitch_nm=max(nm_from_mm("0.25"), repair_options.pitch_nm // 2),
            )
            attempt = _route_net(
                board, _build_grid(board, refined_options, net.pads, fanout_accesses),
                net.name, net.pads, rules.get(net.name), guides.get(net.name),
                usage, {}, clearance, refined_options,
                fanout_accesses=fanout_accesses,
            )
        if not attempt.result.connected:
            selected[net.name] = attempt
            continue
        selected[net.name] = attempt
        for resource in attempt.resources:
            usage[resource] = usage.get(resource, 0) + 1
        for track in attempt.tracks:
            clearance.add_track(track)
        for via in attempt.vias:
            clearance.add_via(via)

    ordinary_names = {net.name for net in nets}
    attempts = tuple(selected[net.name] for net in nets) + tuple(
        item for item in best.nets if item.result.net not in ordinary_names
    )
    overflow = [value - 1 for value in usage.values() if value > 1]
    metrics = DetailedRoutingMetrics(
        routed_net_count=sum(item.result.connected for item in attempts),
        unrouted_net_count=sum(not item.result.connected for item in attempts),
        conflict_resource_count=len(overflow),
        total_conflict_overflow=sum(overflow),
        track_count=sum(len(item.tracks) for item in attempts if item.result.connected),
        via_count=sum(len(item.vias) for item in attempts if item.result.connected),
        total_length_nm=sum(item.result.length_nm for item in attempts if item.result.connected),
        passes=best.metrics.passes,
    )
    return _Pass(attempts, usage, metrics)


def _ripup_orders(blockers: set[str]) -> tuple[tuple[str, ...], ...]:
    """Bound order search, prioritizing reproducibility over factorial work."""

    ordered = tuple(sorted(blockers))
    if len(ordered) <= 3:
        return tuple(permutations(ordered))
    return (ordered, tuple(reversed(ordered)), *(
        ordered[index:] + ordered[:index]
        for index in range(1, min(len(ordered), 4))
    ))


def _attempt_usage(attempts: Iterable[_NetAttempt]) -> dict[str, int]:
    usage: dict[str, int] = {}
    for attempt in attempts:
        if attempt.result.connected:
            for resource in attempt.resources:
                usage[resource] = usage.get(resource, 0) + 1
    return usage


def _route_net(
    board: PhysicalBoard,
    grid: _Grid,
    name: str,
    pads: tuple[PadReference, ...],
    rule: NetRoutingRule | None,
    guide: GlobalNetRoute | None,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    clearance: RoutingClearanceIndex,
    options: DetailedRouterOptions,
    *,
    allow_movable_conflicts: bool = False,
    fanout_accesses: Mapping[PadReference, Point] | None = None,
    forbidden_via_positions: frozenset[Point] = frozenset(),
    via_repair_round: int = 0,
) -> _NetAttempt:
    options = _net_search_options(options, len(pads))
    if guide is None or not guide.connected:
        return _failed(name, "missing connected global guide")
    allowed = routing_layers(board, name, rule)
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    access_options: list[tuple[PadReference, Point, tuple[DetailedNode, ...]]] = []
    for pad in sorted(pads):
        anchor = (fanout_accesses or {}).get(pad)
        if anchor is not None:
            via = next((item for item in board.vias
                        if item.net == name and item.position == anchor), None)
            original = _pad_position(board, pad)
            stub = next((item for item in board.tracks
                         if item.net == name
                         and ((item.start == original and item.end == anchor)
                              or (item.end == original and item.start == anchor))), None)
            if via is None or stub is None:
                return _failed(name, f"unverified fanout anchor for {pad.component}.{pad.pad}")
            layer_indexes = tuple(range(
                grid.layers.index(via.from_layer), grid.layers.index(via.to_layer) + 1,
            ))
            candidates = tuple(
                DetailedNode(layer_index, grid.xs.index(anchor.x_nm), grid.ys.index(anchor.y_nm))
                for layer_index, layer in enumerate(grid.layers)
                if layer in allowed and layer_index in layer_indexes
                and anchor.x_nm in grid.xs and anchor.y_nm in grid.ys
                and DetailedNode(layer_index, grid.xs.index(anchor.x_nm), grid.ys.index(anchor.y_nm)) not in grid.blocked
            )
            pad_position = anchor
        else:
            pad_position = _pad_position(board, pad)
            candidates = _access_candidates(
                board, grid, pad, pad_position, allowed, clearance, name, width,
                options.pin_access_candidates, allow_movable_conflicts,
            )
        if not candidates:
            return _failed(name, f"no legal pin access for {pad.component}.{pad.pad}")
        access_options.append((pad, pad_position, candidates))
    if options.constrained_pins_first and len(access_options) > 2:
        access_options.sort(key=lambda item: (len(item[2]), item[0]))
    tree: set[DetailedNode] = set()
    chosen_accesses: list[tuple[PadReference, Point, DetailedNode]] = []
    remaining = access_options[1:]
    route_edges: set[tuple[DetailedNode, DetailedNode]] = set()
    deviations = 0
    orthogonal_mode_used = not options.octilinear_search
    while remaining:
        starts = tree if tree else set(access_options[0][2])
        target_entry = min(
            remaining,
            key=lambda item: (
                len(item[2]) if options.constrained_pins_first else 0,
                min(_heuristic(grid, candidate, node, options)
                    for candidate in item[2] for node in starts),
                item[0],
            ),
        )
        try:
            found = _search(
                grid,
                starts,
                frozenset(target_entry[2]),
                allowed,
                guide,
                usage,
                history,
                clearance,
                name,
                width,
                options,
                allow_movable_conflicts=allow_movable_conflicts,
                forbidden_via_positions=forbidden_via_positions,
            )
        except _SearchBudgetExceeded:
            if not options.octilinear_search:
                return _failed(
                    name,
                    f"search budget of {options.maximum_search_states} states exhausted "
                    f"for {target_entry[0].component}.{target_entry[0].pad}",
                )
            try:
                found = _search(
                    grid, starts, frozenset(target_entry[2]), allowed,
                    guide, usage, history, clearance, name, width,
                    replace(options, octilinear_search=False, bend_cost=5),
                    allow_movable_conflicts=allow_movable_conflicts,
                    forbidden_via_positions=forbidden_via_positions,
                )
                orthogonal_mode_used = True
            except _SearchBudgetExceeded:
                return _failed(
                    name,
                    f"octilinear and fallback search budgets exhausted "
                    f"for {target_entry[0].component}.{target_entry[0].pad}",
                )
        if found is None:
            return _failed(name, f"detailed search cannot reach {target_entry[0].component}.{target_entry[0].pad}")
        path, root, target = found
        if not tree:
            tree.add(root)
            chosen_accesses.append((access_options[0][0], access_options[0][1], root))
        for first, second, outside in path:
            tree.update((first, second))
            deviations += outside
        tree.add(target)
        chosen_accesses.append((target_entry[0], target_entry[1], target))
        # Shortcuts can remove nodes used later as branch junctions. Keep the
        # exact grid tree for multi-terminal nets until topology-aware cleanup.
        materialized = (
            _compact_path(path, grid, clearance, name, width)
            if options.any_angle_cleanup and len(pads) == 2
            and not allow_movable_conflicts else path
        )
        for first, second, _ in materialized:
            route_edges.add(_edge_key(first, second))
        remaining.remove(target_entry)
    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    via_positions: set[tuple[Point, CopperLayer, CopperLayer, str | None]] = set()
    resources: set[str] = set()
    for first, second in sorted(route_edges):
        if first.layer_index == second.layer_index:
            tracks.append(
                TrackSegment(
                    name,
                    grid.point(first),
                    grid.point(second),
                    width,
                    grid.layers[first.layer_index],
                )
            )
        else:
            via_span = physical_via_span(
                board, grid.layers[first.layer_index], grid.layers[second.layer_index]
            )
            assert via_span is not None
            position = grid.point(first)
            if (position, *via_span) in via_positions:
                continue
            via_positions.add((position, *via_span))
            if any(existing.net == name and existing.position == position
                   and existing.from_layer == via_span[0]
                   and existing.to_layer == via_span[1]
                   for existing in board.vias):
                continue
            vias.append(
                Via(
                    name,
                    position,
                    board.rules.default_via_size_nm,
                    board.rules.default_via_drill_nm,
                    *via_span,
                )
            )
    for first, second in route_edges:
        resources.update(_edge_resources(grid, first, second))
    for _, pad_position, access in chosen_accesses:
        access_position = grid.point(access)
        escape = _access_path(board, clearance, name, pad_position, access_position,
                              width, grid.layers[access.layer_index], allow_movable_conflicts)
        if escape is None:
            return _failed(name, "selected pin access no longer has a legal octilinear path")
        tracks.extend(escape)
    tracks = list(_merge_collinear_tracks(tracks))
    length = sum(
        round(hypot(item.end.x_nm - item.start.x_nm, item.end.y_nm - item.start.y_nm))
        for item in tracks
    )
    conflict = clearance.candidate_via_conflict(
        vias, allow_movable_conflicts=allow_movable_conflicts,
    )
    if conflict is not None:
        if via_repair_round < 2 and conflict.position not in forbidden_via_positions:
            retry = _route_net(
                board, grid, name, pads, rule, guide, usage, history,
                clearance, options,
                allow_movable_conflicts=allow_movable_conflicts,
                fanout_accesses=fanout_accesses,
                forbidden_via_positions=forbidden_via_positions | {conflict.position},
                via_repair_round=via_repair_round + 1,
            )
            if retry.result.connected:
                return retry
        return _failed(name, "candidate vias violate drill spacing or existing copper")
    diagnostics: list[str] = []
    if rule and rule.max_length_nm is not None and length > rule.max_length_nm:
        diagnostics.append(f"route length {length} nm exceeds {rule.max_length_nm} nm")
    if rule and rule.max_vias is not None and len(vias) > rule.max_vias:
        diagnostics.append(f"via count {len(vias)} exceeds {rule.max_vias}")
    return _NetAttempt(
        DetailedNetResult(
            name,
            not diagnostics,
            len(tracks),
            len(vias),
            length,
            deviations,
            tuple(diagnostics),
            orthogonal_mode_used,
        ),
        tuple(tracks),
        tuple(vias),
        frozenset(resources),
    )


def _search(
    grid: _Grid,
    starts: set[DetailedNode],
    targets: frozenset[DetailedNode],
    allowed: tuple[CopperLayer, ...],
    guide: GlobalNetRoute,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
    options: DetailedRouterOptions,
    *,
    allow_movable_conflicts: bool = False,
    forbidden_via_positions: frozenset[Point] = frozenset(),
) -> tuple[tuple[tuple[DetailedNode, DetailedNode, int], ...], DetailedNode, DetailedNode] | None:
    if options.allow_guide_deviation:
        scales = (1, 2, 4) if options.progressive_guides else (1,)
        for scale in scales:
            corridor_budget = min(
                5_000, max(1, options.maximum_search_states // (5 if scale == 1 else 2)),
            )
            try:
                guided = _search_once(
                    grid, starts, targets, allowed, guide, usage, history,
                    clearance, net, width_nm, options,
                    corridor_only=True, state_budget=corridor_budget,
                    guide_margin_nm=options.guide_margin_nm * scale,
                    allow_movable_conflicts=allow_movable_conflicts,
                    forbidden_via_positions=forbidden_via_positions,
                )
            except _SearchBudgetExceeded:
                guided = None
            if guided is not None:
                return guided
        # A global guide reserves coarse capacity, not a mandatory copper
        # layer.  Before searching the whole board, try the same corridor on
        # other *allowed* signal layers.  Physical via legality is still
        # checked at every transition by the search.
        if options.progressive_guides and len(allowed) > 1:
            try:
                projected = _search_once(
                    grid, starts, targets, allowed, guide, usage, history,
                    clearance, net, width_nm, options,
                    corridor_only=True,
                    state_budget=min(5_000, options.maximum_search_states),
                    guide_margin_nm=options.guide_margin_nm * 2,
                    allow_movable_conflicts=allow_movable_conflicts,
                    forbidden_via_positions=forbidden_via_positions,
                    project_guide_layers=True,
                )
            except _SearchBudgetExceeded:
                projected = None
            if projected is not None:
                return projected
    return _search_once(
        grid, starts, targets, allowed, guide, usage, history,
        clearance, net, width_nm, options,
        corridor_only=False, state_budget=options.maximum_search_states,
        guide_margin_nm=options.guide_margin_nm,
        allow_movable_conflicts=allow_movable_conflicts,
        forbidden_via_positions=forbidden_via_positions,
    )


def _search_once(
    grid: _Grid,
    starts: set[DetailedNode],
    targets: frozenset[DetailedNode],
    allowed: tuple[CopperLayer, ...],
    guide: GlobalNetRoute,
    usage: Mapping[str, int],
    history: Mapping[str, int],
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
    options: DetailedRouterOptions,
    *,
    corridor_only: bool,
    state_budget: int,
    guide_margin_nm: int,
    allow_movable_conflicts: bool,
    forbidden_via_positions: frozenset[Point],
    project_guide_layers: bool = False,
) -> tuple[tuple[tuple[DetailedNode, DetailedNode, int], ...], DetailedNode, DetailedNode] | None:
    allowed_indexes = {grid.layers.index(layer) for layer in allowed}
    queue: list[tuple[int, int, int, int, DetailedNode, str, int]] = []
    best: dict[tuple[DetailedNode, str], int] = {}
    previous: dict[tuple[DetailedNode, str], tuple[DetailedNode, str] | None] = {}
    heuristic_cache: dict[DetailedNode, int] = {}
    guide_cache: dict[DetailedNode, bool] = {}
    legal_edge_cache: dict[tuple[DetailedNode, DetailedNode], bool] = {}
    via_legality_cache: dict[tuple[Point, CopperLayer, CopperLayer], tuple[bool, int]] = {}
    movable_edge_cache: dict[tuple[DetailedNode, DetailedNode], int] = {}
    edge_resource_cache: dict[tuple[DetailedNode, DetailedNode], tuple[str, ...]] = {}
    layer_ranks, headings = signal_layer_preferences(grid.board)
    guide_exposure = GuideExposure(guide, guide_margin_nm,
                                   ignore_layer=project_guide_layers)
    guide_edge_costs: dict[tuple[DetailedNode, DetailedNode], int] = {}

    def heuristic(node: DetailedNode) -> int:
        value = heuristic_cache.get(node)
        if value is None:
            value = min(_heuristic(grid, node, target, options) for target in targets)
            heuristic_cache[node] = value
        return value

    def inside_guide(node: DetailedNode) -> bool:
        value = guide_cache.get(node)
        if value is None:
            value = _inside_guide(
                grid, node, guide, guide_margin_nm,
                ignore_layer=project_guide_layers,
            )
            guide_cache[node] = value
        return value

    serial = 0
    for start in sorted(starts):
        if start.layer_index not in allowed_indexes:
            continue
        state = (start, "")
        best[state] = 0
        previous[state] = None
        heappush(queue, (
            options.heuristic_weight_percent * heuristic(start) // 100,
            0, 0, 0, start, "", serial,
        ))
        serial += 1
    final: tuple[DetailedNode, str] | None = None
    expanded = 0
    while queue:
        _, cost, vias, bends, node, direction, _ = heappop(queue)
        state = (node, direction)
        if cost != best.get(state):
            continue
        expanded += 1
        if expanded > state_budget:
            raise _SearchBudgetExceeded
        if node in targets:
            final = state
            break
        for neighbor in _neighbors(
            grid, node, allowed_indexes, diagonals=options.octilinear_search,
        ):
            if corridor_only and neighbor not in targets and not inside_guide(neighbor):
                continue
            next_direction = _direction(node, neighbor)
            if not options.octilinear_search and next_direction != "v":
                next_direction = "h" if node.y_index == neighbor.y_index else "n"
            if next_direction == "v" and grid.point(node) in forbidden_via_positions:
                continue
            edge = _edge_key(node, neighbor)
            legal = legal_edge_cache.get(edge)
            if legal is None:
                movable_count = 0
                if next_direction == "v":
                    legal, movable_count = _cached_via_legality(
                        grid, node, neighbor, clearance, net,
                        allow_movable_conflicts, via_legality_cache,
                    )
                else:
                    if allow_movable_conflicts:
                        movable, locked = clearance.blocking_track_nets(TrackSegment(
                            net, grid.point(node), grid.point(neighbor), width_nm,
                            grid.layers[node.layer_index],
                        ))
                        legal = not locked
                        movable_count = len(movable)
                    else:
                        legal = clearance.can_track(
                            net, grid.point(node), grid.point(neighbor), width_nm,
                            grid.layers[node.layer_index],
                        )
                legal_edge_cache[edge] = legal
                movable_edge_cache[edge] = movable_count
            if not legal:
                continue
            resource_ids = edge_resource_cache.get(edge)
            if resource_ids is None:
                resource_ids = _edge_resources(grid, node, neighbor)
                edge_resource_cache[edge] = resource_ids
            congestion = sum(
                options.present_penalty * max(0, usage.get(resource, 0))
                + options.history_penalty * history.get(resource, 0)
                for resource in resource_ids
            )
            outside = not inside_guide(neighbor)
            if outside and not options.allow_guide_deviation:
                continue
            if next_direction == "v":
                base = options.via_cost * COST_UNIT
                preference = 0
                deviation = guide_transition_cost(inside_guide(node), not outside,
                                                  50 * COST_UNIT)
            else:
                first_point, second_point = grid.point(node), grid.point(neighbor)
                base = length_cost(first_point, second_point)
                layer = grid.layers[node.layer_index]
                deviation = guide_edge_costs.get(edge)
                if deviation is None:
                    deviation = 50 * guide_exposure.outside_length_nm(
                        first_point, second_point, layer)
                    guide_edge_costs[edge] = deviation
                wrong_way = 0
                diagonal = False
                heading = headings.get(layer)
                if heading is not None:
                    if options.octilinear_search:
                        horizontal = next_direction in {"E", "W"}
                        vertical = next_direction in {"N", "S"}
                        if (horizontal and heading != "h"
                                or vertical and heading != "n"):
                            wrong_way = options.direction_preference_cost
                        elif not horizontal and not vertical:
                            wrong_way = options.direction_preference_cost
                            diagonal = True
                    elif next_direction != heading:
                        wrong_way = options.direction_preference_cost
                preference = preference_cost(first_point, second_point, layer_ranks[layer],
                                             options.layer_preference_cost, wrong_way, diagonal)
            if options.octilinear_search:
                bend = _turn_steps(direction, next_direction) * options.bend_cost
            else:
                bend = (
                    options.bend_cost
                    if direction and direction != next_direction
                    and "v" not in {direction, next_direction}
                    else 0
                )
            step = (
                base + preference + deviation + COST_UNIT * (bend + congestion
                + 2 * options.present_penalty * movable_edge_cache[edge])
            )
            candidate_cost = cost + step
            candidate = (neighbor, next_direction)
            if candidate_cost >= best.get(candidate, 1 << 60):
                continue
            best[candidate] = candidate_cost
            previous[candidate] = state
            next_vias = vias + (next_direction == "v")
            next_bends = bends + (bend > 0)
            heappush(
                queue,
                (
                    candidate_cost + (
                        options.heuristic_weight_percent * heuristic(neighbor) // 100
                    ),
                    candidate_cost,
                    next_vias,
                    next_bends,
                    neighbor,
                    next_direction,
                    serial,
                ),
            )
            serial += 1
    if final is None:
        return None
    edges: list[tuple[DetailedNode, DetailedNode, int]] = []
    current = final
    while previous[current] is not None:
        parent = previous[current]
        assert parent is not None
        edges.append(
            (
                parent[0],
                current[0],
                int(not _inside_guide(
                    grid, current[0], guide, guide_margin_nm,
                )),
            )
        )
        current = parent
    edges.reverse()
    return tuple(edges), current[0], final[0]


def _compact_path(
    path: tuple[tuple[DetailedNode, DetailedNode, int], ...],
    grid: _Grid,
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
) -> tuple[tuple[DetailedNode, DetailedNode, int], ...]:
    """Greedily apply a Theta*-style line-of-sight shortcut per layer."""
    if not path:
        return path
    nodes = [path[0][0], *(edge[1] for edge in path)]
    result: list[tuple[DetailedNode, DetailedNode, int]] = []
    start = 0
    while start < len(nodes) - 1:
        end = start + 1
        if nodes[end].layer_index != nodes[start].layer_index:
            result.append((nodes[start], nodes[end], path[start][2]))
            start = end
            continue
        farthest = end
        while end + 1 < len(nodes) and nodes[end + 1].layer_index == nodes[start].layer_index:
            candidate = end + 1
            end = candidate
            if not _octilinear(grid.point(nodes[start]), grid.point(nodes[candidate])):
                continue
            if not _grid_line_clear(grid, nodes[start], nodes[candidate]):
                continue
            if not clearance.can_track(
                net, grid.point(nodes[start]), grid.point(nodes[candidate]),
                width_nm, grid.layers[nodes[start].layer_index],
            ):
                continue
            farthest = candidate
        outside = sum(edge[2] for edge in path[start:farthest])
        result.append((nodes[start], nodes[farthest], outside))
        start = farthest
    return tuple(result)


def _cached_via_legality(
    grid: _Grid, node: DetailedNode, neighbor: DetailedNode,
    clearance: RoutingClearanceIndex, net: str, allow_movable_conflicts: bool,
    cache: dict[tuple[Point, CopperLayer, CopperLayer], tuple[bool, int]],
) -> tuple[bool, int]:
    """Memoize identical physical spans within one immutable, single-net search."""
    span = physical_via_span(grid.board, grid.layers[node.layer_index], grid.layers[neighbor.layer_index])
    if span is None:
        return False, 0
    key = (grid.point(node), *span)
    result = cache.get(key)
    if result is None:
        if allow_movable_conflicts:
            movable, locked = clearance.blocking_via_nets(Via(
                net, key[0], grid.board.rules.default_via_size_nm,
                grid.board.rules.default_via_drill_nm, *span))
            result = not locked, len(movable)
        else:
            result = clearance.can_via(net, key[0], grid.board.rules.default_via_size_nm, *span), 0
        cache[key] = result
    return result


def _grid_line_clear(
    grid: _Grid, start: DetailedNode, end: DetailedNode,
) -> bool:
    board_key = id(grid.board)
    if board_key not in grid.obstacle_cache:
        grid.obstacle_cache[board_key] = (grid.board, tuple(
            item for item in resolved_copper_keepouts(grid.board) if item.block_tracks))
    key = (id(grid.board), start.layer_index, end.layer_index,
           grid.point(start), grid.point(end), grid.blocked, grid.xs, grid.ys)
    result = grid.line_clear_cache.get(key)
    if result is None:
        result = _physical_grid_line_clear(grid, start, end)
        grid.line_clear_cache[key] = result
    return result


def _physical_grid_line_clear(
    grid: _Grid,
    start: DetailedNode,
    end: DetailedNode,
) -> bool:
    """Test the physical ray, never a distorted index-space supercover."""
    if start.layer_index != end.layer_index:
        return False
    first, second = grid.point(start), grid.point(end)
    if not segment_in_polygon(first, second, grid.board.outline.vertices):
        return False
    key = id(grid.board)
    cached = grid.obstacle_cache.get(key)
    if cached is None:
        obstacles = tuple(item for item in resolved_copper_keepouts(grid.board) if item.block_tracks)
        # Hold the immutable board so id reuse cannot alias a replaced grid.
        grid.obstacle_cache[key] = (grid.board, obstacles)
    else:
        obstacles = cached[1]
    ray = RoundedConvexShape((first, second))
    for obstacle in obstacles:
        if (grid.layers[start.layer_index] in obstacle.layers
                and ray.bounds.intersects(RoundedConvexShape(obstacle.outline.outer.vertices).bounds)
                and shape_distance_squared(ray, RoundedConvexShape(obstacle.outline.outer.vertices)) == 0):
            return False
    # Respect synthetic/on-ray blocked nodes too. Off-ray coordinates are
    # irrelevant. Width/clearance are still checked by RoutingClearanceIndex.
    dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
    if dx:
        for ix in range(min(start.x_index, end.x_index), max(start.x_index, end.x_index) + 1):
            numerator = (grid.xs[ix] - first.x_nm) * dy
            if numerator % dx:
                continue
            y = first.y_nm + numerator // dx
            iy = bisect_left(grid.ys, y)
            if iy < len(grid.ys) and grid.ys[iy] == y:
                node = DetailedNode(start.layer_index, ix, iy)
                if node not in {start, end} and node in grid.blocked:
                    return False
    else:
        for iy in range(min(start.y_index, end.y_index), max(start.y_index, end.y_index) + 1):
            node = DetailedNode(start.layer_index, start.x_index, iy)
            if node not in {start, end} and node in grid.blocked:
                return False
    return True


def _build_grid(
    board: PhysicalBoard, options: DetailedRouterOptions,
    pads: tuple[PadReference, ...],
    fanout_accesses: Mapping[PadReference, Point] | None = None,
) -> _Grid:
    min_x = min(item.x_nm for item in board.outline.vertices)
    max_x = max(item.x_nm for item in board.outline.vertices)
    min_y = min(item.y_nm for item in board.outline.vertices)
    max_y = max(item.y_nm for item in board.outline.vertices)
    placements = {item.reference: item for item in board.placements}
    pin_points = (
        transformed_pad_position(board, placements[pad.component], pad.pad)
        for pad in pads
    )
    pin_points = tuple(pin_points) + tuple(
        fanout_accesses[pad] for pad in pads
        if fanout_accesses is not None and pad in fanout_accesses
    )
    xs = tuple(sorted(set(range(min_x, max_x + 1, options.pitch_nm)).union(
        point.x_nm for point in pin_points
    )))
    ys = tuple(sorted(set(range(min_y, max_y + 1, options.pitch_nm)).union(
        point.y_nm for point in pin_points
    )))
    # Courtyards are assembly geometry, not copper obstacles. The clearance
    # index checks the actual placed pads and existing copper instead.
    copper_keepouts = tuple(
        (item.layers, item.outline.outer.vertices)
        for item in resolved_copper_keepouts(board)
        if item.block_tracks
    )
    blocked: set[DetailedNode] = set()
    for layer_index, layer in enumerate(board.stackup.copper_layers):
        for x_index, x in enumerate(xs):
            for y_index, y in enumerate(ys):
                point = Point(x, y)
                if not _point_in_polygon(point, board.outline.vertices):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
                elif any(
                    layer in layers and point_in_polygon(point, polygon)
                    for layers, polygon in copper_keepouts
                ):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
    return _Grid(
        tuple(board.stackup.copper_layers), xs, ys, board, frozenset(blocked),
        options.pitch_nm,
    )


def _access_path(
    board: PhysicalBoard, clearance: RoutingClearanceIndex, net: str,
    start: Point, end: Point, width_nm: int, layer: CopperLayer,
    allow_movable_conflicts: bool = False,
) -> tuple[TrackSegment, ...] | None:
    """Check the same exact octilinear escape during selection and emission.

    Try both diagonal/straight orders, then orthogonal corners. Never fall back
    to an oblique chord or snap a terminal; every emitted leg is checked.
    Tentative rip-up may cross removable tracks, never immutable geometry.
    """
    if start == end:
        return ()
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    if not dx or not dy or abs(dx) == abs(dy):
        paths = ((start, end),)
    else:
        diagonal = min(abs(dx), abs(dy))
        sx, sy = (1 if dx > 0 else -1), (1 if dy > 0 else -1)
        paths = (
            (start, Point(start.x_nm + sx * diagonal, start.y_nm + sy * diagonal), end),
            (start, Point(end.x_nm - sx * diagonal, end.y_nm - sy * diagonal), end),
            (start, Point(start.x_nm, end.y_nm), end),
            (start, Point(end.x_nm, start.y_nm), end),
        )
    for points in paths:
        tracks = tuple(TrackSegment(net, a, b, width_nm, layer)
                       for a, b in zip(points, points[1:]) if a != b)
        for track in tracks:
            if not _track_inside_board(board, track.start, track.end, width_nm):
                break
            if allow_movable_conflicts:
                if clearance.blocking_track_nets(track)[1]:
                    break
            elif not clearance.can_track(net, track.start, track.end, width_nm, layer):
                break
        else:
            return tracks
    return None


def _access_candidates(
    board: PhysicalBoard,
    grid: _Grid,
    pad: PadReference,
    position: Point,
    allowed: tuple[CopperLayer, ...],
    clearance: RoutingClearanceIndex,
    net: str,
    width_nm: int,
    limit: int,
    allow_movable_conflicts: bool = False,
) -> tuple[DetailedNode, ...]:
    placement = next(item for item in board.placements if item.reference == pad.component)
    footprint = board.footprints[placement.footprint]
    physical_pad = next(item for item in footprint.pads if item.number == pad.pad)
    if physical_pad.kind is PadKind.SMD:
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        if side not in allowed:
            return ()
        layers = (side,)
    else:
        layers = allowed
    max_distance_nm = max(nm_from_mm(3), 4 * grid.pitch_nm)
    x_min = bisect_left(grid.xs, position.x_nm - max_distance_nm)
    x_max = bisect_left(grid.xs, position.x_nm + max_distance_nm + 1)
    y_min = bisect_left(grid.ys, position.y_nm - max_distance_nm)
    y_max = bisect_left(grid.ys, position.y_nm + max_distance_nm + 1)
    candidates = (
        DetailedNode(layer_index, x_index, y_index)
        for layer_index, layer in enumerate(grid.layers)
        if layer in layers
        for x_index in range(x_min, x_max)
        for y_index in range(y_min, y_max)
    )
    result: list[DetailedNode] = []
    sectors: set[tuple[int, int, CopperLayer]] = set()
    for node in sorted(candidates, key=lambda item: (
        abs(grid.point(item).x_nm - position.x_nm)
        + abs(grid.point(item).y_nm - position.y_nm), item,
    )):
        access = grid.point(node)
        if abs(access.x_nm - position.x_nm) + abs(access.y_nm - position.y_nm) > max_distance_nm:
            break
        if node in grid.blocked:
            continue
        sector = (
            (access.x_nm > position.x_nm) - (access.x_nm < position.x_nm),
            (access.y_nm > position.y_nm) - (access.y_nm < position.y_nm),
            grid.layers[node.layer_index],
        )
        if sector in sectors:
            continue
        escape = _access_path(board, clearance, net, position, access, width_nm,
                              grid.layers[node.layer_index], allow_movable_conflicts)
        if escape is not None:
            result.append(node)
            sectors.add(sector)
            if len(result) == limit:
                break
    return tuple(result)


def _neighbors(
    grid: _Grid, node: DetailedNode, allowed_indexes: set[int],
    *, diagonals: bool = True,
) -> tuple[DetailedNode, ...]:
    result: list[DetailedNode] = []
    for dl, dx, dy in (
        (0, -1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (0, 1, 0),
    ):
        candidate = DetailedNode(node.layer_index + dl, node.x_index + dx, node.y_index + dy)
        if candidate.layer_index not in allowed_indexes:
            continue
        if not (0 <= candidate.x_index < len(grid.xs) and 0 <= candidate.y_index < len(grid.ys)):
            continue
        if candidate in grid.blocked:
            continue
        if not _grid_line_clear(grid, node, candidate):
            continue
        result.append(candidate)
    if diagonals:
        key = (grid.xs, grid.ys, node.x_index, node.y_index)
        successors = grid.diagonal_successors.get(key)
        if successors is None:
            found = []
            # Find the nearest physical 45-degree successor on each ray.
            # Inserted off-grid axes may split one dimension but not the other.
            for dx, dy in ((-1, -1), (-1, 1), (1, -1), (1, 1)):
                ix, iy = node.x_index + dx, node.y_index + dy
                while 0 <= ix < len(grid.xs) and 0 <= iy < len(grid.ys):
                    xdist = abs(grid.xs[ix] - grid.xs[node.x_index])
                    ydist = abs(grid.ys[iy] - grid.ys[node.y_index])
                    if xdist == ydist:
                        found.append((ix, iy))
                        break
                    if xdist < ydist:
                        ix += dx
                    else:
                        iy += dy
            successors = tuple(found)
            grid.diagonal_successors[key] = successors
        for ix, iy in successors:
            candidate = DetailedNode(node.layer_index, ix, iy)
            if (candidate.layer_index in allowed_indexes and candidate not in grid.blocked
                    and _grid_line_clear(grid, node, candidate)):
                result.append(candidate)
    for layer_index in sorted(allowed_indexes):
        if layer_index == node.layer_index:
            continue
        candidate = DetailedNode(layer_index, node.x_index, node.y_index)
        if candidate not in grid.blocked:
            result.append(candidate)
    return tuple(result)


def _inside_guide(
    grid: _Grid,
    node: DetailedNode,
    guide: GlobalNetRoute,
    margin: int,
    *,
    ignore_layer: bool = False,
) -> bool:
    point = grid.point(node)
    layer = grid.layers[node.layer_index]
    return any(
        (ignore_layer or segment.layer is layer)
        and _point_segment_distance(point, segment.start, segment.end)
        <= segment.guide_half_width_nm + margin
        for segment in guide.segments
    ) or any(
        (ignore_layer or access.layer is layer)
        and abs(point.x_nm - access.access_position.x_nm) <= margin
        and abs(point.y_nm - access.access_position.y_nm) <= margin
        for access in guide.accesses
    )


def _edge_resources(
    grid: _Grid, first: DetailedNode, second: DetailedNode,
) -> tuple[str, ...]:
    edge = _edge_key(first, second)
    a, b = grid.point(edge[0]), grid.point(edge[1])
    la, lb = grid.layers[edge[0].layer_index], grid.layers[edge[1].layer_index]
    first_key = f"{la.value}:{a.x_nm}:{a.y_nm}"
    second_key = f"{lb.value}:{b.x_nm}:{b.y_nm}"
    return (
        f"node:{first_key}",
        f"node:{second_key}",
        f"edge:{first_key}:{second_key}",
    )


def _edge_key(
    first: DetailedNode, second: DetailedNode
) -> tuple[DetailedNode, DetailedNode]:
    return (first, second) if first < second else (second, first)


def _direction(first: DetailedNode, second: DetailedNode) -> str:
    if first.layer_index != second.layer_index:
        return "v"
    dx = (second.x_index > first.x_index) - (second.x_index < first.x_index)
    dy = (second.y_index > first.y_index) - (second.y_index < first.y_index)
    return {
        (1, 0): "E", (1, -1): "NE", (0, -1): "N", (-1, -1): "NW",
        (-1, 0): "W", (-1, 1): "SW", (0, 1): "S", (1, 1): "SE",
    }[(dx, dy)]


def _turn_steps(first: str, second: str) -> int:
    if not first or "v" in {first, second}:
        return 0
    headings = ("E", "NE", "N", "NW", "W", "SW", "S", "SE")
    difference = abs(headings.index(first) - headings.index(second))
    return min(difference, 8 - difference)


def _octilinear(first: Point, second: Point) -> bool:
    dx = abs(second.x_nm - first.x_nm)
    dy = abs(second.y_nm - first.y_nm)
    return dx == 0 or dy == 0 or dx == dy


def _net_search_options(
    options: DetailedRouterOptions, pad_count: int,
) -> DetailedRouterOptions:
    if options.octilinear_search and pad_count >= options.orthogonal_first_min_pads:
        return replace(options, octilinear_search=False, bend_cost=5)
    return options


def _merge_collinear_tracks(tracks: Iterable[TrackSegment]) -> tuple[TrackSegment, ...]:
    """Coalesce exact straight runs without moving copper or branch points."""

    active = list(tracks)
    while True:
        endpoints: dict[tuple[CopperLayer, Point], list[int]] = {}
        for index, track in enumerate(active):
            endpoints.setdefault((track.layer, track.start), []).append(index)
            endpoints.setdefault((track.layer, track.end), []).append(index)
        merge: tuple[int, int, TrackSegment] | None = None
        for (layer, junction), indexes in sorted(
            endpoints.items(),
            key=lambda item: (item[0][0].value, item[0][1].x_nm, item[0][1].y_nm),
        ):
            if len(indexes) != 2:
                continue
            first_index, second_index = indexes
            first, second = active[first_index], active[second_index]
            if first.net != second.net or first.width_nm != second.width_nm:
                continue
            outer_first = first.end if first.start == junction else first.start
            outer_second = second.end if second.start == junction else second.start
            first_dx = junction.x_nm - outer_first.x_nm
            first_dy = junction.y_nm - outer_first.y_nm
            second_dx = outer_second.x_nm - junction.x_nm
            second_dy = outer_second.y_nm - junction.y_nm
            if (first_dx * second_dy != first_dy * second_dx
                    or first_dx * second_dx + first_dy * second_dy <= 0):
                continue
            merge = (first_index, second_index, TrackSegment(
                first.net, outer_first, outer_second, first.width_nm, layer,
            ))
            break
        if merge is None:
            return tuple(active)
        first_index, second_index, joined = merge
        active = [
            track for index, track in enumerate(active)
            if index not in {first_index, second_index}
        ]
        active.append(joined)


def _heuristic(
    grid: _Grid, first: DetailedNode, second: DetailedNode,
    options: DetailedRouterOptions,
) -> int:
    first_point, second_point = grid.point(first), grid.point(second)
    dx = abs(first_point.x_nm - second_point.x_nm)
    dy = abs(first_point.y_nm - second_point.y_nm)
    if options.octilinear_search:
        track_cost = 10 * max(dx, dy) + 4 * min(dx, dy)
    else:
        track_cost = 10 * (dx + dy)
    return track_cost + (options.via_cost * COST_UNIT if first.layer_index != second.layer_index else 0)


def _pad_position(board: PhysicalBoard, pad: PadReference) -> Point:
    placement = next(item for item in board.placements if item.reference == pad.component)
    return transformed_pad_position(board, placement, pad.pad)


def _net_span(board: PhysicalBoard, pads: tuple[PadReference, ...]) -> int:
    points = [_pad_position(board, pad) for pad in pads]
    return (max(item.x_nm for item in points) - min(item.x_nm for item in points)) + (max(item.y_nm for item in points) - min(item.y_nm for item in points))


def _failed(net: str, diagnostic: str) -> _NetAttempt:
    return _NetAttempt(
        DetailedNetResult(net, False, 0, 0, 0, 0, (diagnostic,)),
        (),
        (),
        frozenset(),
    )


def _bounds(points: Iterable[Point]) -> tuple[int, int, int, int]:
    values = tuple(points)
    return min(item.x_nm for item in values), min(item.y_nm for item in values), max(item.x_nm for item in values), max(item.y_nm for item in values)


def _in_box(point: Point, box: tuple[int, int, int, int]) -> bool:
    return box[0] <= point.x_nm <= box[2] and box[1] <= point.y_nm <= box[3]


def _point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if (first.y_nm > point.y_nm) != (second.y_nm > point.y_nm):
            crossing = (second.x_nm - first.x_nm) * (point.y_nm - first.y_nm) / (second.y_nm - first.y_nm) + first.x_nm
            if point.x_nm < crossing:
                inside = not inside
    return inside


def _point_segment_distance(point: Point, start: Point, end: Point) -> float:
    dx = end.x_nm - start.x_nm
    dy = end.y_nm - start.y_nm
    if dx == 0 and dy == 0:
        return hypot(point.x_nm - start.x_nm, point.y_nm - start.y_nm)
    fraction = max(0.0, min(1.0, ((point.x_nm - start.x_nm) * dx + (point.y_nm - start.y_nm) * dy) / (dx * dx + dy * dy)))
    return hypot(point.x_nm - (start.x_nm + fraction * dx), point.y_nm - (start.y_nm + fraction * dy))


def _fingerprint(
    global_fingerprint: str,
    board: PhysicalBoard,
    metrics: DetailedRoutingMetrics,
) -> str:
    document = {
        "global": global_fingerprint,
        "tracks": [
            (item.net, item.layer.value, item.start.x_nm, item.start.y_nm, item.end.x_nm, item.end.y_nm, item.width_nm)
            for item in board.tracks
        ],
        "vias": [
            (item.net, item.position.x_nm, item.position.y_nm, item.from_layer.value, item.to_layer.value, item.size_nm, item.drill_nm)
            for item in board.vias
        ],
        "quality": metrics.quality_vector,
    }
    return sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()
