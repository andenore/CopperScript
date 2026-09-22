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


@dataclass(frozen=True, slots=True)
class CleanupResult:
    board: PhysicalBoard
    removed_track_count: int
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


def shove_bundle(
    board: PhysicalBoard,
    track_indexes: tuple[int, ...],
    via_indexes: tuple[int, ...],
    delta: Point,
    *,
    locked_track_indexes: frozenset[int] = frozenset(),
    locked_via_indexes: frozenset[int] = frozenset(),
) -> ShoveResult:
    """Translate a coupled bundle and every encountered net atomically.

    Expanding collisions by complete net preserves pair/bus geometry instead of
    independently distorting one member of a coupled route.
    """
    moved_tracks, moved_vias = set(track_indexes), set(via_indexes)
    if moved_tracks & locked_track_indexes or moved_vias & locked_via_indexes:
        return ShoveResult(False, board, (), ("bundle contains a locked object",))
    known_nets = {
        *(board.tracks[index].net for index in moved_tracks),
        *(board.vias[index].net for index in moved_vias),
    }
    changed = True
    while changed:
        changed = False
        candidate_tracks = [
            _translated(track, delta) if index in moved_tracks else track
            for index, track in enumerate(board.tracks)
        ]
        candidate_vias = [
            replace(via, position=Point(via.position.x_nm + delta.x_nm,
                                        via.position.y_nm + delta.y_nm))
            if index in moved_vias else via
            for index, via in enumerate(board.vias)
        ]
        for index, track in enumerate(candidate_tracks):
            if index not in moved_tracks:
                continue
            if not all(point_in_polygon(p, board.outline.vertices) for p in (track.start, track.end)):
                return ShoveResult(False, board, (), ("bundle crosses board outline",))
            for other_index, other in enumerate(candidate_tracks):
                if other_index in moved_tracks or other.layer != track.layer or other.net == track.net:
                    continue
                if capsules_clear(track.start, track.end, track.width_nm // 2,
                                  other.start, other.end, other.width_nm // 2,
                                  board.rules.minimum_clearance_nm):
                    continue
                if other_index in locked_track_indexes:
                    return ShoveResult(False, board, (), (f"bundle reaches locked track {other_index}",))
                known_nets.add(other.net)
                changed = True
        for index, via in enumerate(candidate_vias):
            if index not in moved_vias:
                continue
            if not point_in_polygon(via.position, board.outline.vertices):
                return ShoveResult(False, board, (), ("bundle via crosses board outline",))
            for other_index, other in enumerate(candidate_vias):
                if other_index in moved_vias or other.net == via.net:
                    continue
                required = (via.size_nm + other.size_nm) // 2 + board.rules.minimum_clearance_nm
                distance = ((via.position.x_nm - other.position.x_nm) ** 2
                            + (via.position.y_nm - other.position.y_nm) ** 2)
                if distance >= required * required:
                    continue
                if other_index in locked_via_indexes:
                    return ShoveResult(False, board, (), (f"bundle reaches locked via {other_index}",))
                known_nets.add(other.net)
                changed = True
        next_tracks = {i for i, item in enumerate(board.tracks) if item.net in known_nets}
        next_vias = {i for i, item in enumerate(board.vias) if item.net in known_nets}
        if next_tracks != moved_tracks or next_vias != moved_vias:
            moved_tracks, moved_vias = next_tracks, next_vias
            changed = True
    candidate_tracks = tuple(
        _translated(track, delta) if index in moved_tracks else track
        for index, track in enumerate(board.tracks)
    )
    candidate_vias = tuple(
        replace(via, position=Point(via.position.x_nm + delta.x_nm,
                                    via.position.y_nm + delta.y_nm))
        if index in moved_vias else via
        for index, via in enumerate(board.vias)
    )
    return ShoveResult(True, replace(board, tracks=candidate_tracks, vias=candidate_vias),
                       tuple(sorted(moved_tracks)), (), tuple(sorted(moved_vias)))


def cleanup_acute_angles(board: PhysicalBoard) -> CleanupResult:
    """Remove safe degree-two spikes sharper than 45 degrees."""
    tracks = list(board.tracks)
    removed = 0
    changed = True
    while changed:
        changed = False
        for first_index, first in enumerate(tracks):
            for second_index in range(first_index + 1, len(tracks)):
                second = tracks[second_index]
                if first.net != second.net or first.layer != second.layer:
                    continue
                shared = {first.start, first.end}.intersection({second.start, second.end})
                if len(shared) != 1:
                    continue
                junction = next(iter(shared))
                outer_first = first.end if first.start == junction else first.start
                outer_second = second.end if second.start == junction else second.start
                ax, ay = outer_first.x_nm - junction.x_nm, outer_first.y_nm - junction.y_nm
                bx, by = outer_second.x_nm - junction.x_nm, outer_second.y_nm - junction.y_nm
                dot = ax * bx + ay * by
                if dot <= 0 or 2 * dot * dot <= (ax * ax + ay * ay) * (bx * bx + by * by):
                    continue
                replacement = TrackSegment(first.net, outer_first, outer_second,
                                           max(first.width_nm, second.width_nm), first.layer)
                if any(
                    other.net != replacement.net
                    and other.layer == replacement.layer
                    and not capsules_clear(replacement.start, replacement.end,
                                           replacement.width_nm // 2,
                                           other.start, other.end, other.width_nm // 2,
                                           board.rules.minimum_clearance_nm)
                    for index, other in enumerate(tracks)
                    if index not in {first_index, second_index}
                ):
                    continue
                tracks[first_index] = replacement
                del tracks[second_index]
                removed += 1
                changed = True
                break
            if changed:
                break
    return CleanupResult(replace(board, tracks=tuple(tracks)), removed)
