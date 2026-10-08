"""Guide-aware deterministic general detailed router."""

from __future__ import annotations

from bisect import bisect_left
from collections import Counter, OrderedDict
from dataclasses import asdict, dataclass, field, replace
from enum import Enum
from functools import partial
from hashlib import sha256
from heapq import heappop, heappush
from itertools import count as _count, permutations
import json
from math import hypot, isqrt
from time import perf_counter
from types import MappingProxyType
from typing import Iterable, Mapping

from .geometry import (RoundedConvexShape, orientation, point_on_segment, point_in_polygon,
                       segment_in_polygon, shape_distance_squared)
from .route_style import chamfer_ordinary_corners
from .route_cleanup import EscapeChain, prune_track_stubs, release_unused_escapes
from .route_smoothing import smooth_owned_tracks
from .mechanical import point_in_material, shape_in_board
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
from .progress import ProgressCallback, emit
from .routing_clearance import RoutingClearanceIndex
from .breakout import BreakoutRegions
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_vias import physical_via_span
from .routing_costs import COST_UNIT, length_cost, preference_cost
from .routing_guides import GuideExposure, guide_transition_cost
from .surface_path import _track_inside_board
from .pin_escape import (RoutingAccess, access_position, launch_position, checked_access_path,
                         verified_fanout_path, verified_routing_access)


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
    resumed_branch_count: int = 0


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
                    "resumed_branch_count": item.resumed_branch_count,
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
    guide_escape_nm: int = 0
    route_smoothing: bool = False
    # A reserved package escape becomes one alternative terminal of its pad
    # (see _escape_terminals); unused escape copper is released afterwards.
    escape_terminals: bool = False
    maximum_ripup_blockers: int = 4
    minimum_repair_pitch_nm: int = nm_from_mm("0.1")
    # Bound of the per-run memo of exactly repeated repair searches; 0 disables
    # it. A hit returns the identical earlier attempt, never different copper.
    search_reuse_entries: int = 128

    def __post_init__(self) -> None:
        if min(
            self.pitch_nm, self.maximum_passes, self.pin_access_candidates,
            self.maximum_search_states,
            self.minimum_repair_pitch_nm,
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
            self.guide_escape_nm,
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
        if self.search_reuse_entries < 0:
            raise ValueError("detailed search reuse bound cannot be negative")


@dataclass(frozen=True, slots=True)
class _Grid:
    layers: tuple[CopperLayer, ...]
    xs: tuple[int, ...]
    ys: tuple[int, ...]
    board: PhysicalBoard
    blocked: frozenset[DetailedNode]
    pitch_nm: int
    diagonal_successors: dict[tuple[int, ...], tuple[tuple[int, int], ...]] = field(
        default_factory=dict, compare=False, repr=False,
    )
    obstacle_cache: dict[int, tuple[PhysicalBoard, tuple[tuple[CopperKeepout, RoundedConvexShape], ...]]] = field(
        default_factory=dict, compare=False, repr=False,
    )
    line_clear_cache: dict[tuple[object, ...], bool] = field(
        default_factory=dict, compare=False, repr=False,
    )
    query_contexts: dict[tuple[int, int, int], tuple[object, ...]] = field(
        default_factory=dict, compare=False, repr=False,
    )
    _query_context: tuple[int, int, int] = field(init=False, compare=False, repr=False)
    # Node coordinates, built once per grid instance (never shared by a
    # replaced grid, whose axes may differ).
    _points: dict[tuple[int, int], Point] = field(
        default_factory=dict, init=False, compare=False, repr=False,
    )

    def __post_init__(self) -> None:
        key = (id(self.xs), id(self.ys), id(self.blocked))
        object.__setattr__(self, "_query_context", key)
        # Replaced grids share query caches. Retain each immutable input once,
        # rather than constructing its identity/retention tuples on every edge.
        self.query_contexts.setdefault(key, (self.xs, self.ys, self.blocked))

    def point(self, node: DetailedNode) -> Point:
        key = (node.x_index, node.y_index)
        point = self._points.get(key)
        if point is None:
            point = self._points[key] = Point(self.xs[node.x_index], self.ys[node.y_index])
        return point


@dataclass(frozen=True, slots=True)
class _CheckpointGrid:
    """Mesh identity only: do not retain failed searches' large query caches."""

    layers: tuple[CopperLayer, ...]
    xs: tuple[int, ...]
    ys: tuple[int, ...]


@dataclass(frozen=True, slots=True)
class _BranchCheckpoint:
    """Private tentative tree, never output copper or committed congestion."""

    source: PhysicalBoard = field(compare=False, repr=False)
    grid: _CheckpointGrid = field(compare=False, repr=False)
    net: str
    pads: tuple[PadReference, ...]
    allowed: tuple[CopperLayer, ...]
    width_nm: int
    anchors: tuple[tuple[PadReference, Point | RoutingAccess], ...]
    tree: frozenset[DetailedNode]
    chosen: tuple[tuple[PadReference, Point, DetailedNode], ...]
    internal: tuple[tuple[DetailedNode, tuple[PadReference, Point, DetailedNode]], ...]
    remaining: frozenset[PadReference]
    edges: frozenset[tuple[DetailedNode, DetailedNode]]
    deviations: int
    orthogonal_mode_used: bool


@dataclass(frozen=True, slots=True)
class _NetAttempt:
    result: DetailedNetResult
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...]
    resources: frozenset[str]
    # Search provenance, not physical geometry or a schema field. A fine-grid
    # candidate can displace neighbours whose launches need that same mesh.
    pitch_nm: int | None = field(default=None, compare=False, repr=False)
    checkpoint: _BranchCheckpoint | None = field(default=None, compare=False, repr=False)


@dataclass(frozen=True, slots=True)
class _Pass:
    nets: tuple[_NetAttempt, ...]
    usage: Mapping[str, int]
    metrics: DetailedRoutingMetrics


class _SearchBudgetExceeded(Exception):
    """A bounded detailed search exhausted its configured state budget."""


class _Same:
    """Identity key part that retains its object, so the id cannot be reused."""

    __slots__ = ("value",)

    def __init__(self, value: object) -> None:
        self.value = value

    def __eq__(self, other: object) -> bool:
        return isinstance(other, _Same) and other.value is self.value

    def __hash__(self) -> int:
        return id(self.value)


class _SearchReuse:
    """Bounded memo of exactly repeated net searches in one run's repair stage.

    The key is every search input: the immutable board and branch checkpoint
    by identity, the clearance index by its board, bin size and ordered
    insertions, and net, rule, guide, anchors, usage, history, mode and options
    by value. A failure is thus reused only against identical blocking copper.
    The search is deterministic, so a hit returns exactly the recomputed
    attempt. Counters are telemetry, never routing, report or fingerprint data.

    Blocked grid nodes are a pure function of the board and both grid axes, so
    a bounded side table also serves repeated grids, e.g. refinement meshes
    of a blocker that fails in several rip-up orders.
    """

    _GRID_ENTRIES = 16

    def __init__(self, maximum_entries: int) -> None:
        self.maximum_entries = maximum_entries
        self._entries: OrderedDict[tuple[object, ...], tuple[_NetAttempt, float]] = OrderedDict()
        self._blocked: OrderedDict[tuple[object, ...], frozenset[DetailedNode]] = OrderedDict()
        self.hits = 0
        self.misses = 0
        self.evictions = 0
        self.saved_seconds = 0.0
        self.grid_hits = 0

    def lookup(self, key: tuple[object, ...]) -> tuple[_NetAttempt, float] | None:
        entry = self._entries.get(key)
        if entry is None:
            self.misses += 1
            return None
        self._entries.move_to_end(key)
        self.hits += 1
        self.saved_seconds += entry[1]
        return entry

    def store(self, key: tuple[object, ...], attempt: _NetAttempt, seconds: float) -> None:
        self._entries[key] = (attempt, seconds)
        self._entries.move_to_end(key)
        while len(self._entries) > self.maximum_entries:
            self._entries.popitem(last=False)
            self.evictions += 1

    def __len__(self) -> int:
        return len(self._entries)

    def blocked_nodes(self, board: PhysicalBoard, xs: tuple[int, ...],
                      ys: tuple[int, ...]) -> frozenset[DetailedNode] | None:
        if not self.maximum_entries:
            return None
        blocked = self._blocked.get((_Same(board), xs, ys))
        if blocked is not None:
            self._blocked.move_to_end((_Same(board), xs, ys))
            self.grid_hits += 1
        return blocked

    def store_blocked_nodes(self, board: PhysicalBoard, xs: tuple[int, ...],
                            ys: tuple[int, ...], blocked: frozenset[DetailedNode]) -> None:
        if self.maximum_entries:
            self._blocked[_Same(board), xs, ys] = blocked
            while len(self._blocked) > min(self.maximum_entries, self._GRID_ENTRIES):
                self._blocked.popitem(last=False)


def _search_key(
    board: PhysicalBoard, net: PhysicalNet, rule: NetRoutingRule | None,
    guide: GlobalNetRoute | None, usage: Mapping[str, int], history: Mapping[str, int],
    clearance: RoutingClearanceIndex, options: DetailedRouterOptions,
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess], repair: bool,
    allow_movable_conflicts: bool, resume: _BranchCheckpoint | None,
) -> tuple[object, ...]:
    """Exact identity of one ``_search_detailed_net`` call; see ``_SearchReuse``."""

    return (
        _Same(board), _Same(clearance.board), clearance.bin_size_nm, clearance.additions(),
        net, rule, guide, options, tuple(sorted(fanout_accesses.items())),
        tuple(sorted(usage.items())), tuple(sorted(history.items())),
        repair, allow_movable_conflicts, None if resume is None else _Same(resume),
    )


# Telemetry only: equal-content boards share a token, so repeated searches can
# be counted across routing runs. Bounded; holds boards against id reuse.
_IDENTITY_BOARDS: list[tuple[PhysicalBoard, int]] = []
_IDENTITY_BOARD_LIMIT = 16
_IDENTITY_BOARD_TOKENS = _count(1)


def _board_identity_token(board: PhysicalBoard) -> int:
    for known, token in _IDENTITY_BOARDS:
        if known is board:
            return token
    for known, token in _IDENTITY_BOARDS:
        if known.tracks == board.tracks and known.vias == board.vias and known == board:
            break
    else:
        token = next(_IDENTITY_BOARD_TOKENS)
    _IDENTITY_BOARDS.append((board, token))
    del _IDENTITY_BOARDS[:-_IDENTITY_BOARD_LIMIT]
    return token


def _search_identity(key: tuple[object, ...]) -> str:
    """Process-local content token of a search key, for progress telemetry."""

    content = tuple(
        (_board_identity_token(part.value) if isinstance(part.value, PhysicalBoard)
         else (_board_identity_token(part.value.source), part.value, part.value.grid))
        if isinstance(part, _Same) else part
        for part in key
    )
    return f"{hash(content) & 0xFFFF_FFFF_FFFF_FFFF:016x}"


def route_detailed(
    board: PhysicalBoard,
    global_route: GlobalRoutingResult,
    options: DetailedRouterOptions | None = None,
    *,
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess] | None = None,
    fanout_created_vias: frozenset[tuple[str, Point]] | None = None,
    fanout_created_tracks: tuple[TrackSegment, ...] | None = None,
    only_nets: frozenset[str] | None = None,
    on_progress: ProgressCallback | None = None,
) -> DetailedRoutingResult:
    """Route ordinary nets, optionally a repair subset, preserving locked copper.

    Optional progress describes tentative searches, including rejected repair
    work. It is not part of the result, fingerprint or closure evidence.
    """

    options = options or DetailedRouterOptions()
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        raise ValueError("materialize hard-macro copper before detailed routing")
    from .pad_via_arrays import require_via_in_pad_arrays
    require_via_in_pad_arrays(board, "detailed routing")
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
        emit(on_progress, "detailed_pass", "started", pass_index=pass_index,
             ordinary_nets=len(general_nets), deferred_zone_nets=len(deferred))
        completed_passes = pass_index
        pass_options = replace(
            options,
            heuristic_weight_percent=_pass_heuristic_weight(options, pass_index),
        )
        clearance = RoutingClearanceIndex(board)
        usage: dict[str, int] = {}
        failed = ({item.result.net for item in best.nets if not item.result.connected}
                  if best is not None else set())
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
        for net_index, net in enumerate(ordered_nets, 1):
            guide = guides.get(net.name)
            # A failed-first pass must reserve legal narrow launches before
            # ordinary neighbours close them. Fine repair after all other
            # copper is committed is too late for some package corridors.
            repair = pass_index >= 3 and net.name in failed
            attempt = _search_detailed_net(
                board,
                net,
                rules.get(net.name),
                guide,
                usage,
                history,
                clearance,
                pass_options,
                fanout_accesses=fanout_accesses,
                repair=repair, on_progress=on_progress,
                stage="failed_first" if repair else "pass",
                pass_index=pass_index, net_index=net_index, net_count=len(ordered_nets),
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
        emit(on_progress, "detailed_pass", "finished", pass_index=pass_index,
             failed_ordinary_nets=[item.result.net for item in attempts
                 if not item.result.connected and item.result.net not in zone_nets],
             deferred_zone_nets=len(deferred), conflict_overflow=metrics.total_conflict_overflow)
        completed.append(current)
        if best is None or current.metrics.quality_vector < best.metrics.quality_vector:
            best = current
        # Zone nets intentionally await native refill, not another maze pass.
        # Stop once every ordinary signal is connected without conflicts;
        # deferred zones remain explicitly partial until independent closure.
        if metrics.unrouted_net_count == len(deferred) and metrics.total_conflict_overflow == 0:
            best = current
            break
        for resource, value in usage.items():
            if value > 1:
                history[resource] = history.get(resource, 0) + value - 1
    assert best is not None
    if len(completed) > 1 and best.metrics.unrouted_net_count:
        emit(on_progress, "detailed_repair", "started")
        reuse = _SearchReuse(options.search_reuse_entries)
        repaired = _repair_from_passes(
            board, general_nets, rules, guides, best, completed, options,
            fanout_accesses, on_progress=on_progress, reuse=reuse,
        )
        improved = repaired.metrics.quality_vector < best.metrics.quality_vector
        emit(on_progress, "detailed_repair", "finished", selected=improved,
             failed_ordinary_nets=[item.result.net for item in repaired.nets
                 if not item.result.connected and item.result.net not in zone_nets])
        if reuse.maximum_entries:
            emit(on_progress, "detailed_search_reuse", "summary", hits=reuse.hits,
                 misses=reuse.misses, evictions=reuse.evictions, retained=len(reuse),
                 grid_hits=reuse.grid_hits, maximum_entries=reuse.maximum_entries,
                 saved_seconds_estimate=round(reuse.saved_seconds, 3))
        if improved:
            best = repaired
    # Layer/direction preferences must not strand a signal merely because
    # they changed search ordering within a finite state budget. Compare a
    # neutral-cost reroute only when an ordinary net was left open; zone nets
    # deliberately deferred to fill are not a search failure.
    neutral_metrics = None
    layer_ranks, preferred_headings = signal_layer_preferences(board)
    effective_preferences = (
        options.layer_preference_cost and any(layer_ranks.values())
        or options.direction_preference_cost and bool(preferred_headings)
    )
    if (effective_preferences
            and any(not item.result.connected and item.result.net not in zone_nets
                    for item in best.nets)):
        emit(on_progress, "detailed_neutral_fallback", "started")
        neutral = route_detailed(
            board, global_route,
            replace(options, layer_preference_cost=0,
                    direction_preference_cost=0),
            fanout_accesses=fanout_accesses,
            fanout_created_vias=fanout_created_vias,
            fanout_created_tracks=fanout_created_tracks,
            only_nets=only_nets,
            on_progress=on_progress,
        )
        neutral_metrics = neutral.metrics
        improved = (neutral.metrics.unrouted_net_count,
                neutral.metrics.total_conflict_overflow) < (
                best.metrics.unrouted_net_count,
                best.metrics.total_conflict_overflow)
        emit(on_progress, "detailed_neutral_fallback", "finished", selected=improved)
        if improved:
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
    if options.route_smoothing and not fanout_accesses:
        all_tracks = (*board.tracks, *_smooth_owned(board, board.tracks, new_tracks, all_vias))
    if fanout_accesses:
        # Deferred zone contacts remain required even without an area route.
        deferred_pads = {pad for net in deferred for pad in net.pads}
        cleanup_accesses = {pad: anchor for pad, anchor in fanout_accesses.items()
                            if pad not in deferred_pads}
        if only_nets is not None:
            # A failed repair must not remove the input net's locked escape.
            successful_pads = {pad for net in board.nets
                               if any(item.result.net == net.name and item.result.connected
                                      for item in best.nets) for pad in net.pads}
            cleanup_accesses = {pad: anchor for pad, anchor in cleanup_accesses.items()
                                if pad in successful_pads}
        all_tracks, all_vias = _prune_fanout_copper(
            board, all_tracks, all_vias, cleanup_accesses,
            frozenset(item.result.net for item in best.nets if item.result.connected),
            fanout_created_vias,
            fanout_created_tracks,
            smooth=options.route_smoothing, release=options.escape_terminals,
        )
    routed = replace(
        board,
        tracks=all_tracks,
        vias=all_vias,
        metadata=MappingProxyType(metadata),
    )
    # Report accepted copper, not pre-cleanup tentative segments. Occurrence
    # subtraction preserves the established immutable-prefix metric convention.
    actual_tracks = Counter(routed.tracks) - Counter(board.tracks)
    actual_vias = Counter(routed.vias) - Counter(board.vias)
    net_results = tuple(replace(
        item.result,
        track_count=sum(count for track, count in actual_tracks.items() if track.net == item.result.net),
        via_count=sum(count for via, count in actual_vias.items() if via.net == item.result.net),
        length_nm=sum(round(hypot(track.end.x_nm - track.start.x_nm,
                                 track.end.y_nm - track.start.y_nm)) * count
                      for track, count in actual_tracks.items() if track.net == item.result.net),
    ) for item in best.nets)
    metrics = replace(metrics,
        track_count=sum(item.track_count for item in net_results),
        via_count=sum(item.via_count for item in net_results),
        total_length_nm=sum(item.length_nm for item in net_results))
    fingerprint = _fingerprint(global_route.routing_fingerprint, routed, metrics)
    return DetailedRoutingResult(
        DetailedRoutingStatus.SUCCESS if success else DetailedRoutingStatus.PARTIAL,
        routed,
        _with_search_policy(net_results, zone_nets,
                            options, best.metrics, neutral_metrics, False),
        metrics,
        len(board.tracks),
        len(board.vias),
        global_route.routing_fingerprint,
        fingerprint,
    )


def _search_detailed_net(
    board: PhysicalBoard, net: PhysicalNet, rule: NetRoutingRule | None,
    guide: GlobalNetRoute | None, usage: Mapping[str, int], history: Mapping[str, int],
    clearance: RoutingClearanceIndex, options: DetailedRouterOptions, *,
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess],
    repair: bool = True, allow_movable_conflicts: bool = False,
    resume: _BranchCheckpoint | None = None,
    on_progress: ProgressCallback | None = None, stage: str,
    reuse: _SearchReuse | None = None,
    **context: object,
) -> _NetAttempt:
    """Observe one tentative net attempt, starting before grid construction.

    No per-state callbacks or changes to search ordering/budgets. A repair may
    refine the pitch or try several branch searches; this is not a global state
    cap. Interrupted attempts deliberately retain an unmatched start event.
    An exactly repeated search is answered from ``reuse`` (see _SearchReuse).
    """
    emit(on_progress, "detailed_net", "started", net=net.name, stage=stage,
         pad_count=len(net.pads), pitch_nm=options.pitch_nm,
         maximum_search_states=options.maximum_search_states,
         layer_preference_cost=options.layer_preference_cost,
         direction_preference_cost=options.direction_preference_cost,
         heuristic_weight_percent=options.heuristic_weight_percent,
         allow_movable_conflicts=allow_movable_conflicts, **context)
    reuse = reuse if reuse is not None and reuse.maximum_entries else None
    key = (_search_key(board, net, rule, guide, usage, history, clearance, options,
                       fanout_accesses, repair, allow_movable_conflicts, resume)
           if reuse is not None or on_progress is not None else None)
    telemetry: dict[str, object] = {}
    if on_progress is not None:
        telemetry["search_identity"] = _search_identity(key)
    cached = reuse.lookup(key) if reuse is not None else None
    if cached is not None:
        attempt = cached[0]
        telemetry.update(search_reuse="hit", reused_search_seconds=round(cached[1], 3))
    else:
        started = perf_counter()
        grid = _build_grid(board, options, net.pads, fanout_accesses, reuse)
        search = partial(_route_repair_net, reuse=reuse) if repair else _route_net
        attempt = search(board, grid, net.name, net.pads, rule, guide, usage, history,
            clearance, options, fanout_accesses=fanout_accesses,
            allow_movable_conflicts=allow_movable_conflicts, resume=resume)
        if reuse is not None:
            reuse.store(key, attempt, perf_counter() - started)
            telemetry["search_reuse"] = "miss"
    emit(on_progress, "detailed_net", "finished", net=net.name, stage=stage,
         connected=attempt.result.connected, track_count=len(attempt.tracks),
         via_count=len(attempt.vias), diagnostics=list(attempt.result.diagnostics),
         resumed_branch_count=attempt.result.resumed_branch_count,
         final_pitch_nm=attempt.pitch_nm or options.pitch_nm, **telemetry, **context)
    return attempt


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
    accesses: Mapping[PadReference, Point | RoutingAccess],
    successful_nets: frozenset[str],
    created_vias: frozenset[tuple[str, Point]] | None,
    created_tracks: tuple[TrackSegment, ...] | None = None,
    *, smooth: bool = False, release: bool = False,
) -> tuple[tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Prune only explicitly owned lead-ins and unused owned anchor vias.

    With ``smooth``, the same owned copper is then straightened by
    ``route_smoothing``; immutable copper is never an input to that pass.
    With ``release`` (R18), owned land/via/witness copper of a reserved
    boundary access that a successful net does not need is removed first
    (``release_unused_escapes``), and the rest of it may lose dead-end tails
    to stub pruning; it is still never smoothed.
    """

    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads}
    abandoned_nets: set[str] = set()
    anchors: set[tuple[str, Point]] = set()
    for pad, anchor in accesses.items():
        net = net_by_pad[pad]
        anchors.add((net, launch_position(anchor)))
        if net not in successful_nets:
            abandoned_nets.add(net)
    if created_tracks is None:
        # Without ownership evidence, no input lead-in (one or two legs) is
        # disposable. A failed subset already excludes its anchors above.
        owned = Counter()
    else:
        owned = Counter(created_tracks)
    retained = []
    for track in tracks:
        if owned[track] and track.net in abandoned_nets:
            owned[track] -= 1
        else:
            retained.append(track)
    tracks = tuple(retained)
    released: set[tuple[str, Point]] = set()
    settled: set[str] = set()
    if release and created_tracks is not None:
        clearance = RoutingClearanceIndex(board)
        chains = []
        for pad, anchor in sorted(accesses.items()):
            net = net_by_pad[pad]
            if not isinstance(anchor, RoutingAccess) or net not in successful_nets:
                continue
            chain = verified_routing_access(board, pad, net, anchor, clearance)
            if chain is None:
                continue
            via = next((item for item in vias if item.net == net
                        and item.position == anchor.launch_position
                        and (net, item.position) in (created_vias or frozenset())), None)
            chains.append(EscapeChain(net, chain[:len(chain) - len(anchor.path)], anchor.path, via))
            # Exact contact checks below decided this via; a centre-line test
            # would miss a route touching its barrel off-centre.
            released.add((net, anchor.launch_position))
        # The settled copper must match the pruning below: new route copper
        # and owned escape copper are disposable, as are new vias and owned
        # launch vias of the released nets.
        mutable = Counter(track for track in tracks if track.net in successful_nets)
        mutable -= Counter(board.tracks) - owned
        removable = Counter(via for via in vias if via.net in successful_nets) - Counter(board.vias)
        removable.update(chain.via for chain in chains if chain.via is not None)
        tracks, vias = release_unused_escapes(
            board, tracks, vias, tuple(chains), owned, mutable_tracks=mutable,
            removable_vias=removable, clearance=clearance, settled=settled)
    used_layers: dict[tuple[str, Point], set[CopperLayer]] = {
        anchor: set() for anchor in anchors
    }
    for track in tracks:
        for key in used_layers:
            net, point = key
            if track.net == net and point_on_segment(point, track.start, track.end):
                used_layers[key].add(track.layer)
    prunable = (anchors & (created_vias or frozenset())) - released
    vias = tuple(via for via in vias
                 if (via.net, via.position) not in prunable
                 or len(used_layers[(via.net, via.position)]) >= 2)
    # A successful area path can meet a lead-in before its old anchor, leaving
    # an overlapping out-and-back tail. Cleanup owns new copper and explicitly
    # supplied fanout occurrences only; identical locked occurrences survive.
    locked = Counter(board.tracks)
    reserved_nets = {net_by_pad[pad] for pad, anchor in accesses.items()
                     if isinstance(anchor, RoutingAccess)}
    reserved = Counter()
    for track in created_tracks or ():
        if track.net in successful_nets and locked[track]:
            if track.net not in reserved_nets:
                locked[track] -= 1
            elif track.net in settled:
                # Retained escape copper: prunable tails, never smoothed.
                locked[track] -= 1
                reserved[track] += 1
    immutable, mutable = [], []
    for track in tracks:
        if locked[track]:
            locked[track] -= 1
            immutable.append(track)
        elif track.net in successful_nets:
            mutable.append(track)
        else:
            immutable.append(track)
    mutable = prune_track_stubs(
        replace(board, tracks=tuple(immutable), vias=vias), tuple(mutable),
    )
    if reserved:
        originals = tuple(reserved)
        routed = []
        for track in mutable:
            if reserved[track]:
                reserved[track] -= 1
                immutable.append(track)
            elif any(item.net == track.net and item.layer is track.layer
                     and item.width_nm == track.width_nm
                     and point_on_segment(track.start, item.start, item.end)
                     and point_on_segment(track.end, item.start, item.end)
                     for item in originals):
                immutable.append(track)  # A pruned remnant of escape copper.
            else:
                routed.append(track)
        mutable = tuple(routed)
    breakout = BreakoutRegions(board)
    if breakout.ordinary:
        immutable, mutable = _neck_down_changed(board, immutable, mutable, vias, breakout)
    if smooth:
        mutable = _smooth_owned(board, tuple(immutable), mutable, vias)
    return (*immutable, *mutable), vias


def _neck_down_changed(
    board: PhysicalBoard, immutable: list[TrackSegment], mutable: tuple[TrackSegment, ...],
    vias: tuple[Via, ...], breakout: BreakoutRegions,
) -> tuple[list[TrackSegment], tuple[TrackSegment, ...]]:
    """Re-cut pruned copper of ordinary breakout nets at region boundaries.

    Release and stub pruning keep only parts of tracks; part of a wide piece
    can end up inside a region and then carries the breakout width (plan R1).
    Owned route copper and pruned escape remnants are cut, input occurrences
    never. A net whose explicit-copper islands or via contacts would change
    keeps its copper: a released escape may rely on a route's full width.
    """
    from .route_smoothing import _contacts_preserved

    inputs = Counter(board.tracks)
    changed = [False] * len(immutable)
    for index, track in enumerate(immutable):
        if inputs[track]:
            inputs[track] -= 1
        else:
            changed[index] = True  # A pruned remnant of escape copper.
    copper = [track for index, track in enumerate(immutable) if changed[index]] + list(mutable)
    fixed = replace(board, tracks=tuple(track for index, track in enumerate(immutable)
                                        if not changed[index]), vias=vias)
    nets = {net.name: net for net in board.nets}
    accepted: set[str] = set()
    for name in sorted({track.net for track in copper} & breakout.ordinary):
        before = tuple(track for track in copper if track.net == name)
        after = breakout.neck_down(before)
        if after != before and name in nets and _contacts_preserved(fixed, nets[name], before, after):
            accepted.add(name)
    if not accepted:
        return immutable, mutable

    def cut(tracks):
        return [piece for track in tracks
                for piece in (breakout.neck_down((track,)) if track.net in accepted else (track,))]
    return ([piece for index, track in enumerate(immutable)
             for piece in (cut((track,)) if changed[index] else (track,))],
            tuple(cut(mutable)))


def _smooth_owned(
    board: PhysicalBoard, immutable: tuple[TrackSegment, ...],
    owned: tuple[TrackSegment, ...], vias: tuple[Via, ...],
) -> tuple[TrackSegment, ...]:
    """Straighten accepted owned copper against the complete final board."""
    clearance = RoutingClearanceIndex(replace(board, tracks=(*immutable, *owned), vias=vias))
    return smooth_owned_tracks(replace(board, tracks=immutable, vias=vias), owned, clearance)


def _repair_from_passes(
    board: PhysicalBoard,
    nets: list[PhysicalNet],
    rules: Mapping[str, NetRoutingRule],
    guides: Mapping[str, GlobalNetRoute],
    best: _Pass,
    completed: list[_Pass],
    options: DetailedRouterOptions,
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess] | None = None,
    *, on_progress: ProgressCallback | None = None, reuse: _SearchReuse | None = None,
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
    checkpoints: dict[str, _BranchCheckpoint] = {}
    for completed_pass in completed:
        for item in completed_pass.nets:
            checkpoint = item.checkpoint
            if checkpoint is not None and (item.result.net not in checkpoints
                    or len(checkpoint.chosen) > len(checkpoints[item.result.net].chosen)):
                checkpoints[item.result.net] = checkpoint
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
            soft_candidate = _search_detailed_net(
                board, net,
                rules.get(net.name), guides.get(net.name), usage, {}, clearance,
                options, allow_movable_conflicts=True, fanout_accesses=fanout_accesses,
                resume=checkpoints.get(net.name),
                on_progress=on_progress, stage="soft_merge", reuse=reuse,
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
            soft_candidate = _search_detailed_net(
                board, net,
                rules.get(net.name), guides.get(net.name), usage, {}, clearance,
                options, allow_movable_conflicts=True, fanout_accesses=fanout_accesses,
                resume=checkpoints.get(net.name),
                on_progress=on_progress, stage="soft_ripup", reuse=reuse,
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
                eviction_options = replace(options, pitch_nm=max(
                    options.minimum_repair_pitch_nm,
                    min(options.pitch_nm, candidate.pitch_nm or options.pitch_nm)))
                for blocker_name in order:
                    blocker = net_by_name[blocker_name]
                    attempt = _search_detailed_net(
                        board, blocker, rules.get(blocker.name),
                        guides.get(blocker.name), {}, {}, trial_clearance, eviction_options,
                        fanout_accesses=fanout_accesses,
                        on_progress=on_progress, stage="evicted_net", repair_owner=net.name,
                        reuse=reuse,
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
        attempt = _search_detailed_net(
            board, net,
            rules.get(net.name), guides.get(net.name), usage, {}, clearance,
            repair_options, fanout_accesses=fanout_accesses,
            resume=checkpoints.get(net.name),
            on_progress=on_progress, stage="final_retry", reuse=reuse,
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


def _route_repair_net(
    board: PhysicalBoard, grid: _Grid, name: str, pads: tuple[PadReference, ...],
    rule: NetRoutingRule | None, guide: GlobalNetRoute | None,
    usage: Mapping[str, int], history: Mapping[str, int],
    clearance: RoutingClearanceIndex, options: DetailedRouterOptions,
    *, allow_movable_conflicts: bool = False,
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess] | None = None,
    resume: _BranchCheckpoint | None = None, reuse: _SearchReuse | None = None,
) -> _NetAttempt:
    """Bounded fine-grid search shared by every transactional repair stage.

    Refining only the final strict search cannot repair narrow channels blocked
    by movable copper. Soft proposals and each evicted net must have the same
    opportunity, while immutable copper and commit-time clearance remain exact.
    Budget exhaustion never triggers refinement; at most four halvings occur.
    """
    attempt = _route_net(board, grid, name, pads, rule, guide, usage, history,
                         clearance, options, allow_movable_conflicts=allow_movable_conflicts,
                         fanout_accesses=fanout_accesses, resume=resume)
    checkpoint = attempt.checkpoint
    for _ in range(4):
        if (attempt.result.connected
                or not any("cannot reach" in message for message in attempt.result.diagnostics)
                or options.pitch_nm <= options.minimum_repair_pitch_nm):
            break
        options = replace(options,
            pitch_nm=max(options.minimum_repair_pitch_nm, options.pitch_nm // 2))
        attempt = _route_net(
            board, _build_grid(board, options, pads, fanout_accesses, reuse), name, pads,
            rule, guide, usage, history, clearance, options,
            allow_movable_conflicts=allow_movable_conflicts,
            fanout_accesses=fanout_accesses,
        )
        if attempt.checkpoint is not None and (checkpoint is None
                or len(attempt.checkpoint.chosen) > len(checkpoint.chosen)):
            checkpoint = attempt.checkpoint
    return replace(attempt, pitch_nm=options.pitch_nm,
                   checkpoint=None if attempt.result.connected else checkpoint)


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
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess] | None = None,
    forbidden_via_positions: frozenset[Point] = frozenset(),
    via_repair_round: int = 0,
    resume: _BranchCheckpoint | None = None,
) -> _NetAttempt:
    options = _net_search_options(options, len(pads))
    if board.materialized_macros:
        from .drc import explicit_copper_connectivity
        from .hard_macros import macro_routing_pads
        # Preserve an already connected owner net instead of routing a second
        # parallel matching network. This measures explicit copper, not labels.
        net = PhysicalNet(name, pads)
        if explicit_copper_connectivity(board, only_nets=frozenset((name,))).net_connected(net):
            return _NetAttempt(DetailedNetResult(name, True, 0, 0, 0, 0,
                ("connected by immutable input copper",)), (), (), frozenset())
        pads = macro_routing_pads(board, net)
    if guide is None or not guide.connected:
        return _failed(name, "missing connected global guide")
    allowed = routing_layers(board, name, rule)
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    access_options: list[tuple[PadReference, Point, tuple[DetailedNode, ...]]] = []
    access_origins: dict[tuple[PadReference, DetailedNode], Point] = {}
    processed_groups = set()
    interconnected_accesses: set[PadReference] = set()
    escaped: set[PadReference] = set()
    from .hard_macros import macro_routing_ports
    macro_ports = macro_routing_ports(board, name)
    for pad in sorted(pads):
        placement = next(p for p in board.placements if p.reference == pad.component)
        footprint = board.footprints[placement.footprint]
        group = next((g for g in footprint.internal_pad_groups if pad.pad in g.numbers), None)
        if group is not None and pad not in macro_ports:
            key = (pad.component, group.numbers)
            if key in processed_groups:
                continue
            processed_groups.add(key)
        anchor = (fanout_accesses or {}).get(pad)
        if isinstance(anchor, RoutingAccess):
            chain = verified_routing_access(board, pad, name, anchor, clearance)
            if chain is None:
                return _failed(name, f"unverified boundary anchor for {pad.component}.{pad.pad}")
            pad_position = anchor.position
            node = DetailedNode(grid.layers.index(anchor.layer), grid.xs.index(pad_position.x_nm),
                                grid.ys.index(pad_position.y_nm))
            candidates = (node,) if node not in grid.blocked else ()
            if options.escape_terminals:
                escaped.add(pad)
                access_origins.setdefault((pad, node), pad_position)
                alternatives = _escape_terminals(
                    board, grid, name, anchor, chain, allowed, _surface_accesses(
                        board, grid, pad, placement, footprint, group, macro_ports, allowed,
                        clearance, name, width, options, allow_movable_conflicts)[0])
                for candidate, origin in alternatives:
                    access_origins.setdefault((pad, candidate), origin)
                candidates = tuple(dict.fromkeys((*candidates, *(item for item, _ in alternatives))))
                if len(candidates) > 1:
                    interconnected_accesses.add(pad)
        elif anchor is not None:
            via = next((item for item in board.vias
                        if item.net == name and item.position == anchor), None)
            if via is None or verified_fanout_path(board, pad, name, anchor, clearance) is None:
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
            surface, interconnected = _surface_accesses(
                board, grid, pad, placement, footprint, group, macro_ports, allowed,
                clearance, name, width, options, allow_movable_conflicts)
            if interconnected:
                interconnected_accesses.add(pad)
            for candidate, origin in surface:
                access_origins.setdefault((pad, candidate), origin)
            candidates = tuple(dict.fromkeys(candidate for candidate, _ in surface))
        for candidate in candidates:
            access_origins.setdefault((pad, candidate), pad_position)
        if not candidates:
            return _failed(name, f"no legal pin access for {pad.component}.{pad.pad}")
        access_options.append((pad, pad_position, candidates))
    # An escaped pin keeps the priority of its single reserved port: its
    # alternative terminals do not make a crowded package pin unconstrained.
    def constraint(item):
        return 1 if item[0] in escaped else len(item[2])
    if options.constrained_pins_first and len(access_options) > 1:
        access_options.sort(key=lambda item: (constraint(item), item[0]))
    tree: set[DetailedNode] = set()
    chosen_accesses: list[tuple[PadReference, Point, DetailedNode]] = []
    pending_internal_accesses: dict[DetailedNode, tuple[PadReference, Point, DetailedNode]] = {}

    def attach_internal_accesses(entry: tuple[PadReference, Point, tuple[DetailedNode, ...]]) -> None:
        # Only declared conductive groups and the alternative terminals of an
        # escaped pad (R18) create virtual tree roots. Materialize an off-pad
        # access stub only if a later branch uses it.
        if entry[0] not in interconnected_accesses:
            return
        for candidate in entry[2]:
            if candidate not in tree:
                tree.add(candidate)
                pending_internal_accesses.setdefault(candidate,
                    (entry[0], access_origins[entry[0], candidate], candidate))

    remaining = access_options[1:]
    route_edges: set[tuple[DetailedNode, DetailedNode]] = set()
    deviations = 0
    orthogonal_mode_used = not options.octilinear_search
    resumed_branches = 0
    anchors = tuple(sorted((fanout_accesses or {}).items()))
    if (resume is not None and resume.source is board and resume.net == name
            and resume.pads == pads and resume.width_nm == width
            and resume.allowed == tuple(allowed) and resume.anchors == anchors
            and not forbidden_via_positions
            and (resume.grid.layers, resume.grid.xs, resume.grid.ys) == (grid.layers, grid.xs, grid.ys)):
        # Different passes may have placed movable copper over the old tree.
        # Revalidate before reuse; soft proposals may cross only movable owners.
        legal = True
        for first, second in resume.edges:
            if first.layer_index == second.layer_index:
                segment = (name, grid.point(first), grid.point(second), width,
                           grid.layers[first.layer_index])
                legal = (not clearance.route_blockers(*segment)[1] if allow_movable_conflicts
                         else clearance.can_route(*segment))
            else:
                legal = _cached_via_legality(grid, first, second, clearance, name,
                                             allow_movable_conflicts, {})[0]
            if not legal:
                break
        legal &= all(_access_path(board, clearance, name, origin, grid.point(node), width,
                        grid.layers[node.layer_index], allow_movable_conflicts) is not None
                     for _, origin, node in resume.chosen)
        if legal:
            tree = set(resume.tree)
            route_edges = set(resume.edges)
            chosen_accesses = list(resume.chosen)
            pending_internal_accesses = dict(resume.internal)
            remaining = [entry for entry in access_options if entry[0] in resume.remaining]
            deviations = resume.deviations
            orthogonal_mode_used |= resume.orthogonal_mode_used
            resumed_branches = max(0, len({entry[0] for entry in resume.chosen}) - 1)

    def suspend(message: str) -> _NetAttempt:
        failure = _failed(name, message)
        if not tree or not route_edges:
            return failure
        checkpoint = _BranchCheckpoint(board, _CheckpointGrid(grid.layers, grid.xs, grid.ys), name, pads, tuple(allowed), width,
            anchors, frozenset(tree), tuple(chosen_accesses), tuple(pending_internal_accesses.items()),
            frozenset(entry[0] for entry in remaining), frozenset(route_edges), deviations, orthogonal_mode_used)
        return replace(failure, checkpoint=checkpoint,
            result=replace(failure.result, resumed_branch_count=resumed_branches))
    while remaining:
        starts = tree if tree else set(access_options[0][2])
        target_entry = min(
            remaining,
            key=lambda item: (
                constraint(item) if options.constrained_pins_first else 0,
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
                return suspend(
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
                return suspend(
                    f"octilinear and fallback search budgets exhausted "
                    f"for {target_entry[0].component}.{target_entry[0].pad}",
                )
        if found is None:
            return suspend(f"detailed search cannot reach {target_entry[0].component}.{target_entry[0].pad}")
        path, root, target = found
        # Heading-aware/weighted searches can revisit a physical node with a
        # different state. A reconstructed walk is not necessarily a simple
        # copper path; erase its closed excursions before committing edges.
        path = _erase_path_loops(path)
        if escaped and tree:
            # Terminate on touch: a walk that passes another tree node, or a
            # not yet materialized alternative terminal, already connects
            # there. Start from the last such node instead of closing a loop.
            last = max((index for index, (first, _, _) in enumerate(path) if first in tree),
                       default=0)
            path = path[last:]
            if path:
                root = path[0][0]
        internal_root = pending_internal_accesses.pop(root, None)
        if internal_root is not None:
            chosen_accesses.append(internal_root)
        if not tree:
            tree.add(root)
            root_pad = access_options[0][0]
            chosen_accesses.append((root_pad, access_origins[root_pad, root], root))
            attach_internal_accesses(access_options[0])
        for first, second, outside in path:
            tree.update((first, second))
            deviations += outside
        tree.add(target)
        chosen_accesses.append((target_entry[0], access_origins[target_entry[0], target], target))
        attach_internal_accesses(target_entry)
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
    for first, second in sorted(route_edges):
        if first.layer_index == second.layer_index:
            # Exactly the pieces the search checked (necked down near lands).
            tracks.extend(clearance.route_pieces(
                name, grid.point(first), grid.point(second), width,
                grid.layers[first.layer_index],
            ))
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
    for _, pad_position, access in chosen_accesses:
        access_position = grid.point(access)
        escape = _access_path(board, clearance, name, pad_position, access_position,
                              width, grid.layers[access.layer_index], allow_movable_conflicts)
        if escape is None:
            return _failed(name, "selected pin access no longer has a legal octilinear path")
        tracks.extend(escape)
    unpruned = tuple(tracks)
    tracks = list(prune_track_stubs(board, unpruned, tuple(vias), clearance=clearance))
    tracks = list(_merge_collinear_tracks(tracks, clearance.breakout))
    tracks = list(chamfer_ordinary_corners(
        board, tuple(tracks), tuple(vias), clearance,
        allow_movable_conflicts=allow_movable_conflicts,
    ))
    if name in clearance.breakout.ordinary:
        # Pruning and chamfers can leave part of a wide piece inside a region;
        # it necks down too. Every contact here is on a centre line.
        tracks = list(clearance.breakout.neck_down(tracks))
    resources = _emitted_edge_resources(grid, route_edges, tuple(tracks),
        tuple(v for v in (*board.vias, *vias) if v.net == name))
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
            resumed_branch_count=resumed_branches,
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

    escape = options.guide_escape_nm if corridor_only and not project_guide_layers else 0
    terminals: dict[tuple[int, int], list[Point]] = {}
    for node in (*starts, *targets) if escape else ():
        point = grid.point(node)
        terminals.setdefault((point.x_nm // escape, point.y_nm // escape), []).append(point)

    def inside_guide(node: DetailedNode) -> bool:
        value = guide_cache.get(node)
        if value is None:
            value = _inside_guide(
                grid, node, guide, guide_margin_nm,
                ignore_layer=project_guide_layers,
            )
            if not value and escape:
                # A fixed terminal layer may differ from its guide layer.
                # Admit any allowed layer of the projected corridor beside
                # a terminal so the search can change layer there.
                point = grid.point(node)
                column, row = point.x_nm // escape, point.y_nm // escape
                value = (any(max(abs(point.x_nm - item.x_nm), abs(point.y_nm - item.y_nm)) <= escape
                             for dx in (-1, 0, 1) for dy in (-1, 0, 1)
                             for item in terminals.get((column + dx, row + dy), ()))
                         and _inside_guide(grid, node, guide, guide_margin_nm, ignore_layer=True))
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
            # One search has one net and profile width, so an edge determines
            # the widths it is checked at, necked or not (``route_pieces``).
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
                        movable, locked = clearance.route_blockers(
                            net, grid.point(node), grid.point(neighbor), width_nm,
                            grid.layers[node.layer_index],
                        )
                        legal = not locked
                        movable_count = len(movable)
                    else:
                        legal = clearance.can_route(
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


def _erase_path_loops(
    path: tuple[tuple[DetailedNode, DetailedNode, int], ...],
) -> tuple[tuple[DetailedNode, DetailedNode, int], ...]:
    """Erase physical-node excursions in linear time, retaining layer identity."""
    if not path:
        return path
    nodes = [path[0][0]]
    positions = {nodes[0]: 0}
    edges = []
    for first, second, outside in path:
        if first != nodes[-1]:
            raise ValueError("detailed path is not a continuous walk")
        position = positions.get(second)
        if position is not None:
            for node in nodes[position + 1:]:
                del positions[node]
            del nodes[position + 1:]
            del edges[position:]
        else:
            edges.append((first, second, outside))
            positions[second] = len(nodes)
            nodes.append(second)
    return tuple(edges)


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
            if not clearance.can_route(
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


def _grid_query_context(grid: _Grid) -> tuple[int, int, int]:
    """Constant-size cache identity, retaining immutable inputs against ID reuse.

    Replaced grids share caches, so coordinates and blocked nodes are part of
    the identity. Hashing full coordinate tuples on every search edge is not.
    """
    return grid._query_context


def _grid_obstacles(grid: _Grid) -> tuple[tuple[CopperKeepout, RoundedConvexShape], ...]:
    """Placed track obstacles and their shapes, owned by an immutable board.

    A replaced board gets a new snapshot even if it shares this cache. Retaining
    the board prevents ID reuse from aliasing snapshots; mutable routed copper
    is still checked separately by RoutingClearanceIndex on every candidate.
    """
    key = id(grid.board)
    cached = grid.obstacle_cache.get(key)
    if cached is None:
        obstacles = tuple((item, RoundedConvexShape(item.outline.outer.vertices))
                          for item in resolved_copper_keepouts(grid.board) if item.block_tracks)
        grid.obstacle_cache[key] = (grid.board, obstacles)
        return obstacles
    return cached[1]


def _grid_line_clear(
    grid: _Grid, start: DetailedNode, end: DetailedNode,
) -> bool:
    board_key = id(grid.board)
    if board_key not in grid.obstacle_cache:
        _grid_obstacles(grid)
    key = (board_key, *_grid_query_context(grid), start, end)
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
    ray = RoundedConvexShape((first, second))
    if not shape_in_board(grid.board, ray):
        return False
    for obstacle, region in _grid_obstacles(grid):
        if (grid.layers[start.layer_index] in obstacle.layers
                and ray.bounds.intersects(region.bounds)
                and shape_distance_squared(ray, region) == 0):
            return False
    # Respect synthetic/on-ray blocked nodes too. Off-ray coordinates are
    # irrelevant. Width/clearance are still checked by RoutingClearanceIndex.
    dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
    if dx:
        # Endpoints were explicitly ignored below; visit only interior nodes.
        # Adjacent edges therefore allocate no synthetic nodes or endpoint sets.
        for ix in range(min(start.x_index, end.x_index) + 1, max(start.x_index, end.x_index)):
            numerator = (grid.xs[ix] - first.x_nm) * dy
            if numerator % dx:
                continue
            y = first.y_nm + numerator // dx
            iy = bisect_left(grid.ys, y)
            if iy < len(grid.ys) and grid.ys[iy] == y:
                node = DetailedNode(start.layer_index, ix, iy)
                if node in grid.blocked:
                    return False
    else:
        for iy in range(min(start.y_index, end.y_index) + 1, max(start.y_index, end.y_index)):
            node = DetailedNode(start.layer_index, start.x_index, iy)
            if node in grid.blocked:
                return False
    return True


def _build_grid(
    board: PhysicalBoard, options: DetailedRouterOptions,
    pads: tuple[PadReference, ...],
    fanout_accesses: Mapping[PadReference, Point | RoutingAccess] | None = None,
    reuse: _SearchReuse | None = None,
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
    from .hard_macros import macro_routing_ports
    port_points = tuple(port.position for net in board.nets
                        for port in macro_routing_ports(board, net.name).values())
    pin_points = tuple(pin_points) + port_points + tuple(
        access_position(fanout_accesses[pad]) for pad in pads
        if fanout_accesses is not None and pad in fanout_accesses
    )
    if options.escape_terminals and fanout_accesses is not None:
        # A reserved launch via is itself a terminal on each layer it spans.
        pin_points += tuple(fanout_accesses[pad].launch_position for pad in pads
                            if isinstance(fanout_accesses.get(pad), RoutingAccess))
    xs = tuple(sorted(set(range(min_x, max_x + 1, options.pitch_nm)).union(
        point.x_nm for point in pin_points
    )))
    ys = tuple(sorted(set(range(min_y, max_y + 1, options.pitch_nm)).union(
        point.y_nm for point in pin_points
    )))
    # Blocked nodes depend only on the immutable board and both axes. Repeated
    # searches of one net at one pitch share them; query caches stay per grid.
    blocked = reuse.blocked_nodes(board, xs, ys) if reuse is not None else None
    if blocked is None:
        blocked = _blocked_nodes(board, xs, ys)
        if reuse is not None:
            reuse.store_blocked_nodes(board, xs, ys, blocked)
    return _Grid(
        tuple(board.stackup.copper_layers), xs, ys, board, blocked,
        options.pitch_nm,
    )


def _blocked_nodes(
    board: PhysicalBoard, xs: tuple[int, ...], ys: tuple[int, ...],
) -> frozenset[DetailedNode]:
    # Courtyards are assembly geometry, not copper obstacles. The clearance
    # index checks the actual placed pads and existing copper instead.
    copper_keepouts = tuple(
        (item.layers, item.outline.outer.vertices)
        for item in resolved_copper_keepouts(board)
        if item.block_tracks
    )
    # Material is layer-independent: test each point once, not once per layer.
    # A keepout polygon is tested only inside its inclusive bounding box.
    layer_keepouts = tuple(
        tuple((polygon, _bounds(polygon)) for layers, polygon in copper_keepouts
              if layer in layers)
        for layer in board.stackup.copper_layers
    )
    layer_indexes = range(len(layer_keepouts))
    blocked: set[DetailedNode] = set()
    for x_index, x in enumerate(xs):
        for y_index, y in enumerate(ys):
            point = Point(x, y)
            if not point_in_material(board, point):
                blocked.update(DetailedNode(layer_index, x_index, y_index)
                               for layer_index in layer_indexes)
                continue
            for layer_index, keepouts in enumerate(layer_keepouts):
                if any(
                    box[0] <= x <= box[2] and box[1] <= y <= box[3]
                    and point_in_polygon(point, polygon)
                    for polygon, box in keepouts
                ):
                    blocked.add(DetailedNode(layer_index, x_index, y_index))
    return frozenset(blocked)


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
    return checked_access_path(board, clearance, net, start, end, width_nm, layer,
                               allow_movable_conflicts)


def _surface_accesses(
    board: PhysicalBoard, grid: _Grid, pad: PadReference, placement: Placement,
    footprint, group, macro_ports, allowed: tuple[CopperLayer, ...],
    clearance: RoutingClearanceIndex, net: str, width_nm: int,
    options: DetailedRouterOptions, allow_movable_conflicts: bool,
) -> tuple[tuple[tuple[DetailedNode, Point], ...], bool]:
    """A pad's own exact access candidates with their land origins.

    Returns (candidate, origin) pairs in selection order and whether several
    lands of a declared conductive group contribute.
    """
    if group is None or pad in macro_ports:
        origins = ((pad, _pad_position(board, pad), None),)
    else:
        from .placement import transformed_local_point
        origins = tuple((PadReference(pad.component, land.number),
                         transformed_local_point(placement, land.position), land)
                        for land in footprint.pads if land.number in group.numbers)
    result = []
    for physical_reference, origin, land in origins:
        for candidate in _access_candidates(
            board, grid, physical_reference, origin, allowed, clearance, net, width_nm,
            options.pin_access_candidates, allow_movable_conflicts, physical_pad=land,
        ):
            result.append((candidate, origin))
    return tuple(result), len(origins) > 1


def _escape_terminals(
    board: PhysicalBoard, grid: _Grid, net: str, anchor: RoutingAccess,
    chain: tuple[TrackSegment, ...], allowed: tuple[CopperLayer, ...],
    surface: tuple[tuple[DetailedNode, Point], ...],
) -> tuple[tuple[DetailedNode, Point], ...]:
    """Alternative terminals of a pad whose escape is reserved (R18).

    The reserved port is only one way into the pad. Grid nodes on the
    verified land/witness centre lines and the launch via's allowed layers
    touch copper already connected to the pad, so they need no access path;
    the pad's own surface candidates keep their checked lead-ins. Copper
    the final route leaves unused is released by ``release_unused_escapes``.
    """
    result: list[tuple[DetailedNode, Point]] = []
    layer_indexes = {layer: index for index, layer in enumerate(grid.layers)}
    launch = anchor.launch_position
    via = next((item for item in board.vias if item.net == net and item.position == launch), None)
    span = range(layer_indexes[via.from_layer], layer_indexes[via.to_layer] + 1) if via else ()
    x_index, y_index = bisect_left(grid.xs, launch.x_nm), bisect_left(grid.ys, launch.y_nm)
    if (x_index < len(grid.xs) and grid.xs[x_index] == launch.x_nm
            and y_index < len(grid.ys) and grid.ys[y_index] == launch.y_nm):
        for layer in allowed:
            index = layer_indexes[layer]
            if index in span:
                node = DetailedNode(index, x_index, y_index)
                if node not in grid.blocked:
                    result.append((node, launch))
    for track in chain:
        if track.layer not in allowed:
            continue
        index = layer_indexes[track.layer]
        low_x, high_x = sorted((track.start.x_nm, track.end.x_nm))
        low_y, high_y = sorted((track.start.y_nm, track.end.y_nm))
        for xi in range(bisect_left(grid.xs, low_x), bisect_left(grid.xs, high_x + 1)):
            for yi in range(bisect_left(grid.ys, low_y), bisect_left(grid.ys, high_y + 1)):
                point = Point(grid.xs[xi], grid.ys[yi])
                node = DetailedNode(index, xi, yi)
                if node not in grid.blocked and point_on_segment(point, track.start, track.end):
                    result.append((node, point))
    result.extend(surface)
    return tuple(result)


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
    *, physical_pad=None,
) -> tuple[DetailedNode, ...]:
    placement = next(item for item in board.placements if item.reference == pad.component)
    footprint = board.footprints[placement.footprint]
    physical_pad = physical_pad or next(item for item in footprint.pads if item.number == pad.pad)
    from .hard_macros import macro_routing_ports, macro_port_layers
    port = macro_routing_ports(board, net).get(pad)
    if port is not None:
        layers = tuple(layer for layer in macro_port_layers(board, port) if layer in allowed)
    elif physical_pad.kind is PadKind.SMD:
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
        key = (*_grid_query_context(grid), node.x_index, node.y_index)
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


def _emitted_edge_resources(
    grid: _Grid, edges: Iterable[tuple[DetailedNode, DetailedNode]],
    tracks: tuple[TrackSegment, ...], vias: tuple[Via, ...],
) -> frozenset[str]:
    """Keep maze hints only where the owner's final copper supports them.

    Pruning can shorten an edge; chamfering can retire its original corner.
    Resource hints are not an alternate DRC: exact physical clearance still
    checks every candidate, including new diagonal segments and via spans.
    All supplied copper belongs to this one tentative net.
    """
    by_layer = {layer: tuple(t for t in tracks if t.layer is layer)
                for layer in grid.layers}
    spans: dict[Point, list[tuple[int, int]]] = {}
    for via in vias:
        low, high = sorted((grid.layers.index(via.from_layer), grid.layers.index(via.to_layer)))
        spans.setdefault(via.position, []).append((low, high))
    resources: set[str] = set()
    for first, second in edges:
        first, second = _edge_key(first, second)
        a, b = grid.point(first), grid.point(second)
        if first.layer_index != second.layer_index:
            cursor = first.layer_index
            for low, high in sorted(spans.get(a, ())):
                if low > cursor:
                    break
                cursor = max(cursor, high)
            if a == b and cursor >= second.layer_index:
                resources.update(_edge_resources(grid, first, second))
            continue
        selected = by_layer[grid.layers[first.layer_index]]
        axis = "x_nm" if a.x_nm != b.x_nm else "y_nm"
        low, high = sorted((getattr(a, axis), getattr(b, axis)))
        intervals = []
        for track in selected:
            if orientation(a, b, track.start) != 0 or orientation(a, b, track.end) != 0:
                continue
            start = max(low, min(getattr(track.start, axis), getattr(track.end, axis)))
            end = min(high, max(getattr(track.start, axis), getattr(track.end, axis)))
            if start < end:
                intervals.append((start, end))
        if not intervals:
            continue  # A fully retired edge must never contribute maze usage.
        keys = _edge_resources(grid, first, second)
        for point, node, key in ((a, first, keys[0]), (b, second, keys[1])):
            if (any(low <= node.layer_index <= high for low, high in spans.get(point, ()))
                    or any(point_on_segment(point, t.start, t.end) for t in selected)):
                resources.add(key)
        cursor = low
        for start, end in sorted(intervals):
            if end <= cursor:
                continue
            if start > cursor:
                break
            cursor = end
            if cursor >= high:
                resources.add(keys[2])
                break
    return frozenset(resources)


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


def _merge_collinear_tracks(
    tracks: Iterable[TrackSegment], breakout: BreakoutRegions | None = None,
) -> tuple[TrackSegment, ...]:
    """Coalesce exact straight runs without moving copper or branch points.

    With ``breakout``, runs of an ordinary breakout net merge only if they are
    both outside a region, or the joined run is inside one: copper checked at
    a region's values must stay inside it (plan R1).
    """

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
            if breakout is not None and first.net in breakout.ordinary:
                inside = breakout.region(first.net, (first.start, first.end)) is not None
                if (inside != (breakout.region(second.net, (second.start, second.end)) is not None)
                        or inside and breakout.region(first.net, (outer_first, outer_second)) is None):
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
    from .hard_macros import macro_routing_ports
    for net in board.nets:
        if pad in net.pads:
            port = macro_routing_ports(board, net.name).get(pad)
            if port is not None:
                return port.position
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
