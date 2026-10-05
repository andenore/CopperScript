"""Bounded same-layer joint package escape and octilinear pair search.

Both lanes follow one oriented spine. Pad fanout is selected jointly and turns
use offset-line intersections, not independent D+/D- maze routes. This module
proposes copper only: the critical stage owns profile/native-DRC acceptance.
Layer transitions and non-octilinear terminal rows deliberately fail closed.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from fractions import Fraction
from heapq import heappop, heappush
from itertools import product
from math import ceil, hypot, sqrt
from typing import Iterator, NamedTuple

from .geometry import RoundedConvexShape, shapes_clear
from .physical import BoardSide, CopperLayer, NetRoutingRule, PhysicalBoard, Point, TrackSegment, Via, nm_from_mm
from .routing import GlobalNetRoute
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .surface_path import _track_inside_board


_HEADS = ((1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1))


@dataclass(frozen=True, slots=True)
class _Port:
    center: Point
    heading: int  # outward from the package
    sign: int  # first member's side relative to the outward heading
    first: tuple[TrackSegment, ...]
    second: tuple[TrackSegment, ...]
    preference: int


@dataclass(frozen=True, slots=True)
class PairSearchCandidate:
    first: tuple[TrackSegment, ...]
    second: tuple[TrackSegment, ...]
    expanded_states: int
    search_index: int
    spine: tuple[Point, ...] = ()
    start_port: _Port | None = None
    end_port: _Port | None = None
    via_pairs: tuple[tuple[Via, Via], ...] = ()
    return_vias: tuple[Via, ...] = ()


class _SpineSearchResult(NamedTuple):
    first: tuple[TrackSegment, ...]
    second: tuple[TrackSegment, ...]
    expanded_states: int
    spine: tuple[Point, ...]


@dataclass(slots=True)
class PairSearchStats:
    """Search-local telemetry, never copper ownership or acceptance evidence."""
    port_pairs: int = 0
    searches: int = 0
    expanded_states: int = 0
    candidates: int = 0


def _normal(heading: int, offset: int) -> Point:
    dx, dy = _HEADS[heading]
    amount = ceil(offset / sqrt(2)) if dx and dy else offset
    return Point(-dy * amount, dx * amount)


def _shift(point: Point, normal: Point, sign: int) -> Point:
    return Point(point.x_nm + sign * normal.x_nm, point.y_nm + sign * normal.y_nm)


def _miter(point: Point, incoming: int, outgoing: int, offset: int, sign: int) -> Point:
    first, second = _normal(incoming, offset), _normal(outgoing, offset)
    if incoming == outgoing:
        return _shift(point, first, sign)
    ax, ay = _HEADS[incoming]
    bx, by = _HEADS[outgoing]
    determinant = ax * by - ay * bx
    distance = Fraction(sign * ((second.x_nm - first.x_nm) * by
                                - (second.y_nm - first.y_nm) * bx), determinant)
    return Point(point.x_nm + sign * first.x_nm + round(distance * ax),
                 point.y_nm + sign * first.y_nm + round(distance * ay))


def _heading(start: Point, end: Point) -> int | None:
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    if not dx and not dy:
        return None
    if dx and dy and abs(abs(dx) - abs(dy)) > 2:
        return None
    return _HEADS.index(((dx > 0) - (dx < 0), (dy > 0) - (dy < 0)))


def _turn(first: int, second: int) -> int:
    return min((first - second) % 8, (second - first) % 8)


def _paths(start: Point, end: Point) -> tuple[tuple[Point, ...], ...]:
    if start == end:
        return ((start,),)
    if _heading(start, end) is not None:
        return ((start, end),)
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    distance = min(abs(dx), abs(dy))
    sx, sy = (1 if dx > 0 else -1), (1 if dy > 0 else -1)
    return ((start, Point(start.x_nm + sx * distance, start.y_nm + sy * distance), end),
            (start, Point(end.x_nm - sx * distance, end.y_nm - sy * distance), end))


def _tracks(net: str, points: tuple[Point, ...], width: int, layer: CopperLayer) -> tuple[TrackSegment, ...]:
    return tuple(TrackSegment(net, a, b, width, layer)
                 for a, b in zip(points, points[1:]) if a != b)


def _legal(board: PhysicalBoard, index: RoutingClearanceIndex,
           first: tuple[TrackSegment, ...], second: tuple[TrackSegment, ...], clearance: int) -> bool:
    for track in (*first, *second):
        if (_heading(track.start, track.end) is None
                or not _track_inside_board(board, track.start, track.end, track.width_nm)
                or not index.can_track(track.net, track.start, track.end, track.width_nm, track.layer)):
            return False
    return all(shapes_clear(RoundedConvexShape((a.start, a.end), a.width_nm // 2),
                            RoundedConvexShape((b.start, b.end), b.width_nm // 2), clearance)
               for a in first for b in second if a.layer is b.layer)


def _ports(board: PhysicalBoard, index: RoutingClearanceIndex, first_name: str,
           second_name: str, first: Point, second: Point, component: str,
           width: int, offset: int, clearance: int, layer: CopperLayer) -> tuple[_Port, ...]:
    separation = _heading(second, first)
    if separation is None:
        return ()
    middle = Point((first.x_nm + second.x_nm) // 2, (first.y_nm + second.y_nm) // 2)
    placement = next(item for item in board.placements if item.reference == component)
    outward = Point(middle.x_nm - placement.position.x_nm, middle.y_nm - placement.position.y_nm)
    headings = ((separation - 2) % 8, (separation + 2) % 8)
    headings = tuple(sorted(headings, key=lambda h: (
        -(_HEADS[h][0] * outward.x_nm + _HEADS[h][1] * outward.y_nm), h)))
    ports = []
    for preference, heading in enumerate(headings):
        normal = _normal(heading, offset)
        sign = 1 if ((first.x_nm - second.x_nm) * normal.x_nm
                     + (first.y_nm - second.y_nm) * normal.y_nm) > 0 else -1
        for length in (.5, .75, 1, 1.5, 2, 3):
            step = nm_from_mm(length)
            dx, dy = _HEADS[heading]
            center = Point(middle.x_nm + dx * step, middle.y_nm + dy * step)
            targets = (_shift(center, normal, sign), _shift(center, normal, -sign))
            for a, b in product(_paths(first, targets[0]), _paths(second, targets[1])):
                # Fanout must reach the pair port in its stated orientation.
                if (_heading(a[-2], a[-1]) != heading
                        or _heading(b[-2], b[-1]) != heading):
                    continue
                first_tracks, second_tracks = (_tracks(first_name, a, width, layer),
                                               _tracks(second_name, b, width, layer))
                if _legal(board, index, first_tracks, second_tracks, clearance):
                    ports.append(_Port(center, heading, sign, first_tracks, second_tracks, preference))
                    break
    return tuple(ports)


def _lane_paths(points: tuple[Point, ...], incoming: int, outgoing: int,
                offset: int, sign: int) -> tuple[tuple[Point, ...], tuple[Point, ...]] | None:
    points = tuple(p for i, p in enumerate(points) if not i or p != points[i - 1])
    if len(points) < 2:
        return None
    headings = tuple(_heading(a, b) for a, b in zip(points, points[1:]))
    if any(h is None for h in headings):
        return None
    orientations = (incoming, *headings, outgoing)
    if any(_turn(a, b) > 1 for a, b in zip(orientations, orientations[1:])):
        return None
    first, second = [], []
    for index, point in enumerate(points):
        previous = incoming if index == 0 else headings[index - 1]
        following = outgoing if index == len(points) - 1 else headings[index]
        first.append(_miter(point, previous, following, offset, sign))
        second.append(_miter(point, previous, following, offset, -sign))
    # Short spine edges may be consumed by adjacent miters. An offset lane
    # can then collapse or run backward although every spine turn is legal.
    # Reject the whole joint construction, not just one member's tiny edge.
    # Search/refinement must find a different topology within its own bounds.
    if any(_heading(a, b) != heading
           for lane in (first, second)
           for a, b, heading in zip(lane, lane[1:], headings)):
        return None
    return tuple(first), tuple(second)


def paired_candidates(board: PhysicalBoard, first_rule: NetRoutingRule, second_rule: NetRoutingRule,
                      first_guide: GlobalNetRoute, second_guide: GlobalNetRoute, *,
                      maximum_searches: int = 8, maximum_states: int = 30_000,
                      pitch_nm: int = nm_from_mm(1),
                      stats: PairSearchStats | None = None,
                      maximum_total_states: int | None = None) -> Iterator[PairSearchCandidate]:
    """Yield deterministic joint candidates; never reserve or modify input copper."""
    if min(maximum_searches, maximum_states, pitch_nm) <= 0:
        raise ValueError("joint pair search bounds must be positive")
    if maximum_total_states is not None and maximum_total_states <= 0:
        raise ValueError("joint pair aggregate state bound must be positive")
    stats = stats if stats is not None else PairSearchStats()
    if maximum_total_states is not None and stats.expanded_states >= maximum_total_states:
        return
    width = first_rule.width_nm or board.rules.default_track_width_nm
    if (second_rule.width_nm or board.rules.default_track_width_nm) != width:
        return
    gap = first_rule.pair_gap_nm
    if gap is None or second_rule.pair_gap_nm != gap:
        return
    if len(first_guide.accesses) != 2 or len(second_guide.accesses) != 2:
        return
    partners = {access.pad.component: access for access in second_guide.accesses}
    if len(partners) != 2 or set(partners) != {access.pad.component for access in first_guide.accesses}:
        return
    # A guide's access layer can be reached only through its tentative via.
    # Surface-only alternatives start on actual pad-side copper, not an inner
    # grid layer mistaken for a physical terminal contact.
    placements = {p.reference:p for p in board.placements}
    layers = {CopperLayer.FRONT if placements[access.pad.component].side is BoardSide.FRONT
              else CopperLayer.BACK for access in first_guide.accesses}
    if len(layers) != 1:
        return
    layer = next(iter(layers))
    if layer not in set(routing_layers(board, first_rule.net, first_rule)).intersection(
        routing_layers(board, second_rule.net, second_rule)):
        return
    offset = (width + gap + 1) // 2
    clearance = max(board.rules.minimum_clearance_nm, first_rule.clearance_nm or 0,
                    second_rule.clearance_nm or 0)
    index = RoutingClearanceIndex(board)
    accesses = sorted(first_guide.accesses, key=lambda access: access.pad.component)
    groups = [_ports(board, index, first_rule.net, second_rule.net, access.pad_position,
                     partners[access.pad.component].pad_position, access.pad.component,
                     width, offset, clearance, layer) for access in accesses]
    combinations = sorted([(a, b) for a, b in product(*groups) if a.sign == -b.sign],
                          key=lambda pair: (pair[0].preference + pair[1].preference,
                                            hypot(pair[0].center.x_nm - pair[1].center.x_nm,
                                                  pair[0].center.y_nm - pair[1].center.y_nm),
                                            pair[0].center.x_nm, pair[0].center.y_nm,
                                            pair[1].center.x_nm, pair[1].center.y_nm))
    stats.port_pairs += len(combinations)
    for search_index, (start, end) in enumerate(combinations[:maximum_searches], 1):
        budget = _remaining_states(stats, maximum_states, maximum_total_states)
        if not budget:
            break
        escaped = (*start.first, *start.second, *end.first, *end.second)
        if not _legal(board, index, (*start.first, *end.first), (*start.second, *end.second), clearance):
            continue
        escaped_board = replace(board, tracks=(*board.tracks, *escaped))
        stats.searches += 1
        candidate = _search(escaped_board, RoutingClearanceIndex(escaped_board), first_rule.net,
                            second_rule.net, start, end, width, offset, clearance, layer,
                            pitch_nm, budget, stats)
        if candidate is not None:
            stats.candidates += 1
            a, b, expanded, spine = candidate
            yield PairSearchCandidate((*start.first, *a, *tuple(
                TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer) for t in reversed(end.first))),
                (*start.second, *b, *tuple(
                TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer) for t in reversed(end.second))),
                expanded, search_index, spine, start, end)


def _remaining_states(stats: PairSearchStats, maximum_states: int,
                      maximum_total_states: int | None) -> int:
    """A shared counter caps work across ports, layers, pitches and families."""
    return (maximum_states if maximum_total_states is None else
            max(0, min(maximum_states, maximum_total_states - stats.expanded_states)))


def _goal_paths(point: Point, end: Point, heading: int, inward: int,
                pitch: int) -> Iterator[tuple[Point, ...]]:
    """Bounded terminal collars; allow a local S-turn instead of a tiny cusp.

    Retain cheap historical bridges first. Then advance along allowed headings
    before joining the fixed end collar. Two-leg collars can distribute a
    sub-grid displacement across an S-turn without a consumed inner edge.
    Full forward-miter, escape, width/gap and obstacle checks still own legality.
    """
    dx, dy = _HEADS[inward]
    approach = Point(end.x_nm - dx * pitch, end.y_nm - dy * pitch)
    yield from _paths(point, end)
    yield from (path + (end,) for path in _paths(point, approach))
    for direction in (heading, (heading - 1) % 8, (heading + 1) % 8):
        dx, dy = _HEADS[direction]
        for distance in (pitch, 2 * pitch):
            advanced = Point(point.x_nm + dx * distance, point.y_nm + dy * distance)
            for path in _paths(advanced, approach):
                yield (point, *path, end)
    for turn in (-1, 1):
        dx, dy = _HEADS[(heading + turn) % 8]
        fx, fy = _HEADS[heading]
        for distance in (max(1, pitch // 2), pitch):
            first = Point(point.x_nm + dx * distance, point.y_nm + dy * distance)
            second = Point(first.x_nm + fx * distance, first.y_nm + fy * distance)
            for path in _paths(second, approach):
                yield (point, first, *path, end)


def _search(board: PhysicalBoard, index: RoutingClearanceIndex, first_name: str, second_name: str,
            start: _Port, end: _Port, width: int, offset: int, clearance: int,
            layer: CopperLayer, pitch: int, budget: int, stats: PairSearchStats) -> _SpineSearchResult | None:
    inward = (end.heading + 4) % 8
    initial = (start.center, start.heading)
    best = {initial: 0}
    previous = {initial: None}
    serial = 0
    queue = [(0, 0, serial, initial)]
    legal_cache = {}
    expanded = 0

    def legal_step(point: Point, old: int, new: int, initial_step: bool) -> bool:
        key = point, old, new, initial_step
        if key not in legal_cache:
            dx, dy = _HEADS[old]
            behind = Point(point.x_nm - dx * pitch, point.y_nm - dy * pitch)
            dx, dy = _HEADS[new]
            ahead = Point(point.x_nm + dx * pitch, point.y_nm + dy * pitch)
            # The package fanout is already checked and reserved. Extending a
            # fictitious coupled segment behind its port can intersect its
            # taper even though that segment will never be emitted.
            points = (point, ahead) if initial_step else (behind, point, ahead)
            lanes = _lane_paths(points, old, new, offset, start.sign)
            legal_cache[key] = lanes is not None and _legal(
                board, index, _tracks(first_name, lanes[0], width, layer),
                _tracks(second_name, lanes[1], width, layer), clearance)
        return legal_cache[key]

    while queue and expanded < budget:
        _, cost, _, state = heappop(queue)
        if cost != best.get(state):
            continue
        point, heading = state
        expanded += 1
        stats.expanded_states += 1
        if hypot(point.x_nm - end.center.x_nm, point.y_nm - end.center.y_nm) <= 4 * pitch:
            for tail in _goal_paths(point, end.center, heading, inward, pitch):
                spine = []
                cursor = state
                while cursor is not None:
                    spine.append(cursor[0])
                    cursor = previous[cursor]
                points = tuple(reversed(spine)) + tail[1:]
                if (len(points) < 2 or _heading(points[0], points[1]) != start.heading
                        or _heading(points[-2], points[-1]) != inward):
                    continue
                lanes = _lane_paths(points, start.heading, inward, offset, start.sign)
                if lanes is None:
                    continue
                a, b = _tracks(first_name, lanes[0], width, layer), _tracks(second_name, lanes[1], width, layer)
                if _legal(board, index, a, b, clearance):
                    return _SpineSearchResult(a, b, expanded, points)
        for new in ((heading - 1) % 8, heading, (heading + 1) % 8):
            if state == initial and new != heading:
                continue  # The first lane endpoints must meet the exact port.
            if not legal_step(point, heading, new, state == initial):
                continue
            dx, dy = _HEADS[new]
            target = Point(point.x_nm + dx * pitch, point.y_nm + dy * pitch)
            next_state = target, new
            next_cost = cost + round(hypot(dx * pitch, dy * pitch)) + (pitch // 4 if new != heading else 0)
            if next_cost >= best.get(next_state, 10**30):
                continue
            best[next_state] = next_cost
            previous[next_state] = state
            serial += 1
            heuristic = round(hypot(target.x_nm - end.center.x_nm, target.y_nm - end.center.y_nm))
            heappush(queue, (next_cost + heuristic, next_cost, serial, next_state))
    return None
