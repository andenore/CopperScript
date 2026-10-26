"""Detailed routing result types, and land and smoothing helpers the router shares."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field, replace
from enum import Enum
from hashlib import sha256
import json
from math import isqrt

from .geometry import segment_in_polygon
from .route_smoothing import smooth_owned_tracks
from .physical import PadKind, PhysicalBoard, Placement, Point, TrackSegment, Via
from .routing_clearance import RoutingBlocker, RoutingClearanceIndex
from .surface_path import _track_inside_board


class DetailedRoutingStatus(str, Enum):
    SUCCESS = "success"
    PARTIAL = "partial"


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
class DetailedSearchStats:
    """Search work counters: telemetry, never geometry, ordering or signoff.

    One *call* is one bounded A* search for one terminal connection in one
    stage. Stages are tried in order: the guide corridor, wider corridors,
    the corridor projected onto other allowed layers, then the whole board.
    ``failed_expanded_states`` counts states expanded by calls that found no
    path, whether they exhausted their budget or their queue. Counts are
    deterministic for fixed inputs; they never enter comparisons, route
    fingerprints, candidate selection or acceptance.
    """

    search_calls: int = 0
    expanded_states: int = 0
    failed_expanded_states: int = 0
    budget_exhausted_calls: int = 0
    no_path_calls: int = 0
    corridor_successes: int = 0
    widened_corridor_successes: int = 0
    projected_corridor_successes: int = 0
    full_board_successes: int = 0
    maximum_target_count: int = 0


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
    search_budget_exhausted: bool = False
    retry_heuristic_weight_percent: int | None = None
    # Declared length budgets the connected copper misses: review warnings,
    # never a reason to report the net open (drc.ROUTING_BUDGET_WARNING_CODES).
    warnings: tuple[str, ...] = ()
    # Work of the attempt that produced this result; a reused search reports
    # the original attempt's work. Telemetry only (see DetailedSearchStats).
    search_stats: DetailedSearchStats | None = field(default=None, compare=False)


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
class DetailedRepairTrial:
    owner: str
    index_digest: str
    blockers: tuple[RoutingBlocker, ...]
    displaced_nets: tuple[str, ...]
    outcome: str
    orders_tried: int = 0
    reroute_searches: int = 0
    merge_selected: bool = False


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
    repair_trials: tuple[DetailedRepairTrial, ...] = ()
    # All bounded searches of this run, including rejected passes, repair
    # trials and probes. Telemetry only (see DetailedSearchStats).
    search_work: DetailedSearchStats | None = field(default=None, compare=False)
    # Per-tile routing congestion of the run (a congestion_feedback
    # CongestionMap; unified router only, None otherwise). Placement-feedback
    # telemetry: not compared and not part of to_json.
    congestion: object | None = field(default=None, compare=False)

    def to_json(self) -> str:
        document = {
            "schema": "copperscript-detailed-route/v0.1",
            "status": self.status.value,
            "global_routing_fingerprint": self.global_routing_fingerprint,
            "routing_fingerprint": self.routing_fingerprint,
            "repair_trials": [asdict(item) for item in self.repair_trials],
            "repair_trial_report_limit": 256,
            "locked_track_count": self.locked_track_count,
            "locked_via_count": self.locked_via_count,
            "search_work": asdict(self.search_work) if self.search_work else None,
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
                    "warnings": list(item.warnings),
                    "orthogonal_mode_used": item.orthogonal_mode_used,
                    "search_policy": asdict(item.search_policy) if item.search_policy else None,
                    "resumed_branch_count": item.resumed_branch_count,
                    "search_budget_exhausted": item.search_budget_exhausted,
                    "retry_heuristic_weight_percent": item.retry_heuristic_weight_percent,
                    "search_stats": asdict(item.search_stats) if item.search_stats else None,
                }
                for item in self.nets
            ],
        }
        return json.dumps(document, indent=2, sort_keys=True) + "\n"


def _smooth_owned(
    board: PhysicalBoard, immutable: tuple[TrackSegment, ...],
    owned: tuple[TrackSegment, ...], vias: tuple[Via, ...],
) -> tuple[TrackSegment, ...]:
    """Straighten accepted owned copper against the complete final board."""
    clearance = RoutingClearanceIndex(replace(board, tracks=(*immutable, *owned), vias=vias))
    return smooth_owned_tracks(replace(board, tracks=immutable, vias=vias), owned, clearance)


def land_pieces(placement: Placement, footprint, number: str) -> tuple[tuple[tuple[object, Point], ...], ...]:
    """The lands of one pad number grouped into pieces of touching copper.

    The piece holding the first land comes first; each piece lists its lands
    with their board positions in footprint order. A single piece needs no
    route between its lands.
    """
    from .drc import placed_pad_shape
    from .geometry import shapes_clear
    from .placement import transformed_local_point
    lands = [land for land in footprint.pads if land.number == number
             and land.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}]
    if len(lands) < 2:
        return ()
    positions = [transformed_local_point(placement, land.position) for land in lands]
    shapes = [placed_pad_shape(position, land, placement) for land, position in zip(lands, positions)]
    parent = list(range(len(lands)))

    def find(index: int) -> int:
        while parent[index] != index:
            parent[index] = parent[parent[index]]
            index = parent[index]
        return index

    # One footprint's surface lands share its side, so any overlap is contact.
    for first in range(len(lands)):
        for second in range(first + 1, len(lands)):
            if not shapes_clear(shapes[first], shapes[second], 1):
                parent[find(second)] = find(first)
    pieces: dict[int, list[tuple[object, Point]]] = {}
    for index in range(len(lands)):
        pieces.setdefault(find(index), []).append((lands[index], positions[index]))
    return tuple(tuple(piece) for piece in pieces.values())


def _point_in_polygon(point: Point, polygon: tuple[Point, ...]) -> bool:
    inside = False
    for index, first in enumerate(polygon):
        second = polygon[(index + 1) % len(polygon)]
        if (first.y_nm > point.y_nm) != (second.y_nm > point.y_nm):
            crossing = (second.x_nm - first.x_nm) * (point.y_nm - first.y_nm) / (second.y_nm - first.y_nm) + first.x_nm
            if point.x_nm < crossing:
                inside = not inside
    return inside


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
