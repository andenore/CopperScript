"""Non-mutating, bounded surface feed proposals for the power planner prototype.

Uses the existing exact surface router. Proposals are not adopted copper or
native connectivity; a caller must validate the combined board before adoption.
No via, source-zone replacement or board-specific selector is introduced here.
"""
from dataclasses import dataclass

from .physical import BoardSide, CopperLayer, PadReference, PhysicalBoard, RouteKind, TrackSegment
from .drc import physical_board_digest
from .placement import transformed_pad_position
from .routing_clearance import RoutingClearanceIndex
from .surface_path import surface_path_between


@dataclass(frozen=True)
class FeedCorridor:
    requested_width_nm: int
    status: str
    tracks: tuple[TrackSegment, ...] = ()


@dataclass(frozen=True)
class FeedCorridorResult:
    snapshot: str
    net: str
    source: PadReference
    target: PadReference
    candidates: tuple[FeedCorridor, ...]
    state_budget_per_candidate: int


def propose_feed_corridors(board: PhysicalBoard, *, net: str, source: PadReference,
                           target: PadReference, widths_nm: tuple[int, ...],
                           maximum_candidates=4, state_budget=4000,
                           step_nm=125000, maximum_detour_nm=1000000) -> FeedCorridorResult:
    if (any(type(v) is not int or v <= 0 for v in (maximum_candidates, state_budget, step_nm))
            or type(maximum_detour_nm) is not int or maximum_detour_nm < 0):
        raise ValueError('invalid feed proposal bounds')
    widths = tuple(sorted(set(widths_nm)))
    if not widths or len(widths) > maximum_candidates or any(type(w) is not int or w <= 0 for w in widths):
        raise ValueError('invalid or excessive feed width candidates')
    physical_net = next((n for n in board.nets if n.name == net), None)
    if physical_net is None or source == target or not {source, target} <= set(physical_net.pads):
        raise ValueError('feed endpoints must be distinct declared net members')
    if board.hard_macros and set(board.materialized_macros) != {m.cluster for m in board.hard_macros}:
        raise ValueError('materialize immutable macros before feed proposals')
    snapshot = physical_board_digest(board)
    poses = {p.reference: p for p in board.placements}
    rule = next((r for r in board.net_routing_rules if r.net == net), None)
    first, second = poses[source.component], poses[target.component]
    layer = CopperLayer.FRONT if first.side is BoardSide.FRONT else CopperLayer.BACK
    unsupported = ('unsupported_opposite_side' if first.side != second.side else
                   'specialized_route_required' if rule and rule.kind is not RouteKind.GENERAL else
                   'layer_not_allowed' if rule and rule.allowed_layers and layer not in rule.allowed_layers else None)
    minimum = max(board.rules.minimum_track_width_nm, rule.width_nm or 0 if rule else 0)
    clearance = RoutingClearanceIndex(board)
    candidates = []
    for width in widths:
        status = unsupported or ('below_source_width' if width < minimum else None)
        if status:
            candidates.append(FeedCorridor(width, status))
            continue
        tracks = surface_path_between(board, clearance, net,
            transformed_pad_position(board, first, source.pad),
            transformed_pad_position(board, second, target.pad), width, layer,
            step_nm=step_nm, maximum_detour_nm=maximum_detour_nm, state_budget=state_budget)
        candidates.append(FeedCorridor(width, 'proposed' if tracks else 'no_path_within_budget', tracks or ()))
    return FeedCorridorResult(snapshot, net, source, target, tuple(candidates), state_budget)
