"""Transactional deterministic line push-and-shove operations."""

from __future__ import annotations

from dataclasses import dataclass, replace

from .geometry import capsules_clear, point_in_polygon
from .physical import PhysicalBoard, Point, TrackSegment


@dataclass(frozen=True, slots=True)
class ShoveResult:
    committed: bool
    board: PhysicalBoard
    moved_track_indexes: tuple[int, ...]
    diagnostics: tuple[str, ...] = ()


def shove_track(board: PhysicalBoard, track_index: int, delta: Point, *,
                locked_track_indexes: frozenset[int] = frozenset()) -> ShoveResult:
    """Translate a collision graph atomically or return the original board."""
    if not 0 <= track_index < len(board.tracks):
        raise IndexError("track index is outside the board")
    if delta == Point(0, 0):
        return ShoveResult(True, board, ())
    if track_index in locked_track_indexes:
        return ShoveResult(False, board, (), ("requested track is locked",))
    clearance = board.rules.minimum_clearance_nm
    moved, queue, candidates = {track_index}, [track_index], list(board.tracks)
    while queue:
        index = queue.pop(0)
        candidates[index] = _translated(board.tracks[index], delta)
        candidate = candidates[index]
        if not all(point_in_polygon(p, board.outline.vertices) for p in (candidate.start, candidate.end)):
            return ShoveResult(False, board, (), ("shove crosses board outline",))
        for other_index, other in enumerate(candidates):
            if other_index == index or other.layer != candidate.layer or other.net == candidate.net:
                continue
            if capsules_clear(candidate.start, candidate.end, candidate.width_nm // 2,
                              other.start, other.end, other.width_nm // 2, clearance):
                continue
            if other_index in locked_track_indexes:
                return ShoveResult(False, board, (), (f"shove reaches locked track {other_index}",))
            if other_index not in moved:
                moved.add(other_index)
                queue.append(other_index)
    for first_index, first in enumerate(candidates):
        for second in candidates[first_index + 1:]:
            if first.net == second.net or first.layer != second.layer:
                continue
            if not capsules_clear(first.start, first.end, first.width_nm // 2,
                                  second.start, second.end, second.width_nm // 2, clearance):
                return ShoveResult(False, board, (), ("shove leaves a collision",))
    return ShoveResult(True, replace(board, tracks=tuple(candidates)), tuple(sorted(moved)))


def _translated(track: TrackSegment, delta: Point) -> TrackSegment:
    return replace(track,
                   start=Point(track.start.x_nm + delta.x_nm, track.start.y_nm + delta.y_nm),
                   end=Point(track.end.x_nm + delta.x_nm, track.end.y_nm + delta.y_nm))
