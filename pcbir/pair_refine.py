"""Bounded shared-spine shortcuts; the critical owner validates each proposal."""
from __future__ import annotations

from dataclasses import dataclass, replace
from math import hypot
from typing import Iterator

from .pair_search import PairSearchCandidate, _heading, _lane_paths, _legal, _paths, _tracks
from .physical import NetRoutingRule, PhysicalBoard, Point, TrackSegment
from .routing_clearance import RoutingClearanceIndex


@dataclass(slots=True)
class PairRefinementStats:
    attempts: int = 0
    candidates: int = 0


def _compact(points: tuple[Point, ...]) -> tuple[Point, ...]:
    """Remove only exact, forward collinear vertices; preserve both endpoints."""
    result: list[Point] = []
    for point in points:
        if result and point == result[-1]:
            continue
        while len(result) > 1:
            a, b = result[-2:]
            ax, ay = b.x_nm - a.x_nm, b.y_nm - a.y_nm
            bx, by = point.x_nm - b.x_nm, point.y_nm - b.y_nm
            if ax * by != ay * bx or ax * bx + ay * by <= 0:
                break
            result.pop()
        result.append(point)
    return tuple(result)


def _length(tracks: tuple[TrackSegment, ...]) -> int:
    return sum(round(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm)) for t in tracks)


def paired_shortcuts(
    board: PhysicalBoard, candidate: PairSearchCandidate,
    first_rule: NetRoutingRule, second_rule: NetRoutingRule, *,
    maximum_attempts: int = 256, stats: PairRefinementStats | None = None,
) -> Iterator[tuple[tuple[TrackSegment, ...], tuple[TrackSegment, ...]]]:
    """Propose joint octilinear shortcuts without altering ports or input copper.

    Search paths have one spine, not arbitrary branched tracks. Missing spine
    provenance fails closed. Each proposal includes the immutable original pad
    escapes, preserves lane side/order and undergoes full paired geometry checks.
    Profile/native acceptance belongs to the caller; a rejected proposal must
    never replace its accepted incumbent.
    """
    if maximum_attempts <= 0:
        raise ValueError("pair refinement bound must be positive")
    stats = stats if stats is not None else PairRefinementStats()
    if candidate.via_pairs or candidate.return_vias:
        return  # Surface-spine compaction cannot own multilayer transitions.
    start, end = candidate.start_port, candidate.end_port
    if not candidate.spine or start is None or end is None:
        return
    if candidate.spine[0] != start.center or candidate.spine[-1] != end.center:
        return
    width = first_rule.width_nm or board.rules.default_track_width_nm
    gap = first_rule.pair_gap_nm
    if gap is None or second_rule.pair_gap_nm != gap or (second_rule.width_nm or board.rules.default_track_width_nm) != width:
        return
    escaped = (*start.first, *start.second, *end.first, *end.second)
    if not escaped or len({track.layer for track in escaped}) != 1 or start.sign != -end.sign:
        return
    for lane, lead, tail in zip((candidate.first, candidate.second), (start.first, start.second), (end.first, end.second)):
        reversed_tail = tuple(replace(t, start=t.end, end=t.start) for t in reversed(tail))
        if not lead or not tail or lane[:len(lead)] != lead or lane[-len(tail):] != reversed_tail:
            return
    layer = escaped[0].layer
    offset = (width + gap + 1) // 2
    clearance = max(board.rules.minimum_clearance_nm, first_rule.clearance_nm or 0, second_rule.clearance_nm or 0)
    index = RoutingClearanceIndex(replace(board, tracks=(*board.tracks, *escaped)))
    inbound = (end.heading + 4) % 8
    current = candidate.spine
    incumbent = candidate.first, candidate.second

    def materialize(points):
        if len(points) < 2 or _heading(points[0], points[1]) != start.heading or _heading(points[-2], points[-1]) != inbound:
            return None
        lanes = _lane_paths(points, start.heading, inbound, offset, start.sign)
        if lanes is None:
            return None
        a, b = _tracks(first_rule.net, lanes[0], width, layer), _tracks(second_rule.net, lanes[1], width, layer)
        if not _legal(board, index, a, b, clearance):
            return None
        result = ((*start.first, *a, *(replace(t, start=t.end, end=t.start) for t in reversed(end.first))),
                  (*start.second, *b, *(replace(t, start=t.end, end=t.start) for t in reversed(end.second))))
        if not _legal(board, index, *result, clearance):
            return None
        return result

    def improves(proposal):
        before = tuple(_length(lane) for lane in incumbent)
        after = tuple(_length(lane) for lane in proposal)
        return all(a <= b for a, b in zip(after, before)) and (
            sum(after), sum(map(len, proposal))) < (sum(before), sum(map(len, incumbent)))

    # Provenance is structural, not just matching endpoint labels. A stale or
    # unrelated spine must never delete accepted copper outside its own lanes.
    if materialize(current) != incumbent:
        return

    compacted = _compact(current)
    if compacted != current and stats.attempts < maximum_attempts:
        stats.attempts += 1
        proposal = materialize(compacted)
        if proposal is not None and improves(proposal):
            incumbent = proposal
            stats.candidates += 1
            yield proposal
        # Exact forward-collinear spine compaction is geometrically equivalent
        # even if per-segment integer length rounding rejects its metric score.
        current = compacted

    while stats.attempts < maximum_attempts:
        changed = False
        for first in range(len(current) - 2):
            for last in range(len(current) - 1, first + 1, -1):
                for bridge in _paths(current[first], current[last]):
                    points = _compact((*current[:first], *bridge, *current[last + 1:]))
                    if points == current:
                        continue
                    if stats.attempts >= maximum_attempts:
                        return
                    stats.attempts += 1
                    proposal = materialize(points)
                    if proposal is None or not improves(proposal):
                        continue
                    current, incumbent = points, proposal
                    stats.candidates += 1
                    yield proposal
                    changed = True
                    break
                if changed:
                    break
            if changed:
                break
        if not changed:
            return
