"""Transactional deterministic line push-and-shove operations."""

from __future__ import annotations

from dataclasses import dataclass, replace

from .geometry import capsules_clear, point_in_polygon, point_segment_distance_squared
from .physical import PhysicalBoard, Point, TrackSegment, Via


@dataclass(frozen=True, slots=True)
class ShoveResult:
    committed: bool
    board: PhysicalBoard
    moved_track_indexes: tuple[int, ...]
    diagnostics: tuple[str, ...] = ()
    moved_via_indexes: tuple[int, ...] = ()


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


def shove_via(board: PhysicalBoard, via_index: int, delta: Point, *,
              locked_via_indexes: frozenset[int] = frozenset(),
              locked_track_indexes: frozenset[int] = frozenset()) -> ShoveResult:
    """Atomically translate a via collision graph; copper tracks are obstacles."""
    if not 0 <= via_index < len(board.vias):
        raise IndexError("via index is outside the board")
    if via_index in locked_via_indexes:
        return ShoveResult(False, board, (), ("requested via is locked",))
    candidates = list(board.vias)
    moved, queue = {via_index}, [via_index]
    layers = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    while queue:
        index = queue.pop(0)
        source = board.vias[index]
        candidate = replace(source, position=Point(source.position.x_nm + delta.x_nm,
                                                   source.position.y_nm + delta.y_nm))
        if not point_in_polygon(candidate.position, board.outline.vertices):
            return ShoveResult(False, board, (), ("via shove crosses board outline",))
        candidates[index] = candidate
        low, high = sorted((layers[candidate.from_layer], layers[candidate.to_layer]))
        for track_index, track in enumerate(board.tracks):
            if track.net == candidate.net or not low <= layers[track.layer] <= high:
                continue
            required_twice = candidate.size_nm + track.width_nm + 2 * board.rules.minimum_clearance_nm
            if 4 * point_segment_distance_squared(candidate.position, track.start, track.end) < required_twice ** 2:
                label = "locked " if track_index in locked_track_indexes else ""
                return ShoveResult(False, board, (), (f"via shove reaches {label}track {track_index}",))
        for other_index, other in enumerate(candidates):
            if other_index == index or other.net == candidate.net:
                continue
            other_low, other_high = sorted((layers[other.from_layer], layers[other.to_layer]))
            if high < other_low or other_high < low:
                continue
            required = (candidate.size_nm + other.size_nm) // 2 + board.rules.minimum_clearance_nm
            distance_squared = ((candidate.position.x_nm - other.position.x_nm) ** 2
                                + (candidate.position.y_nm - other.position.y_nm) ** 2)
            if distance_squared >= required ** 2:
                continue
            if other_index in locked_via_indexes:
                return ShoveResult(False, board, (), (f"via shove reaches locked via {other_index}",))
            if other_index not in moved:
                moved.add(other_index)
                queue.append(other_index)
    return ShoveResult(True, replace(board, vias=tuple(candidates)), (), (), tuple(sorted(moved)))
