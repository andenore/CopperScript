"""Length-match tuning of accepted critical copper (D-PHY plan R3).

After every critical group is accepted, ``route_critical_nets`` lengthens the
shorter members of each ``length_match`` group whose skew exceeds its
``max_skew``. Tuning works on *units*: one accepted critical group each. A
differential pair is one unit and both members receive the same bumps, bent
together at the pair's own spacing, so the pair's own skew does not change. A
single-ended critical net is a unit of its own.

Corner rule (plan R9). A bump keeps its perpendicular legs, but each corner is
a 45-degree chamfer (``bump_chamfers``). For a bump of height h the member
inside a turn takes leg c = min(width, h // 4, (h - d) // 2) and the other
member c + d, with d = floor((2 - sqrt 2) x spacing) for a pair and 0 for a
single net: the offset chamfers of a coupled 45-degree bend, so parallel pieces
of the lanes stay at least the pair spacing apart (at most 2 nm more). The
member on the bulge side is inside the turn at both run corners and outside it
at both top corners, the other member the reverse, so each gains exactly
``bump_gain``: 2h - 4(2c + d) plus four rounded 45-degree pieces, about
2h - 2(2 - sqrt 2)(2c + d) (2h - 4(2 - sqrt 2)c for a single net). A bump too
low for c >= 1 has one 45-degree ramp per side instead and gains
2(round(h sqrt 2) - h).

Placement rule. Candidate slots lie on straight axis-aligned runs: a coupled
run where both pair members are parallel at the pair spacing, or any
axis-aligned segment of a single net. Slots keep 3 x width from both run ends,
and bumps are 3 x width wide (plus twice the lane spacing for the outer member
of a pair) and 3 x width apart, on a grid centred in the run, bulging to either
side. The *room* of a slot is the tallest bump, at most the amplitude limit,
whose swept area clears foreign copper, lands, keep-outs, holes and the board
edge by the applicable clearance. The swept area is the convex outline of the
bulge-side member's chamfered bump, which holds every member's bump copper
outside the pair's own lanes. Slots are used in order of most room first, then
farthest from the unit's terminals, then by position; the fewest bumps that
provide the length are taken and their heights are levelled. Native DRC still
validates the result atomically.

This module holds the per-group report and the bump geometry. The critical
router owns the order, the length targets and the atomic validation
(``critical.py``).
"""

from __future__ import annotations

from dataclasses import dataclass
from math import hypot, isqrt
from typing import Callable

from .geometry import Bounds, RoundedConvexShape
from .mechanical import shape_in_board
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment
from .routing_clearance import RoutingClearanceIndex


# Bound on serpentine bumps per tuned unit.
MATCH_TUNING_BUMP_LIMIT = 16

# Pair lanes count as coupled within this geometric tolerance, as in
# ``critical._coupled_length``.
_LANE_TOLERANCE_NM = 4


@dataclass(frozen=True, slots=True)
class MatchTuningMember:
    net: str
    length_before_nm: int
    length_after_nm: int

    @property
    def added_length_nm(self) -> int:
        return self.length_after_nm - self.length_before_nm


@dataclass(frozen=True, slots=True)
class MatchTuningResult:
    """Outcome of tuning one ``length_match`` group.

    ``status`` is ``within_limit`` (copper untouched), ``tuned``, ``failed``
    (copper kept exactly as routed; ``reason`` says why) or ``incomplete`` (a
    member is not connected after critical routing, so the group is not judged
    here).
    """

    id: str
    max_skew_nm: int
    status: str
    skew_before_nm: int | None
    skew_after_nm: int | None
    members: tuple[MatchTuningMember, ...]
    bumps: int = 0
    reason: str = ""


def match_tuning_document(item: MatchTuningResult) -> dict[str, object]:
    """JSON-ready form of one group's tuning outcome for the critical report."""
    return {
        "id": item.id,
        "max_skew_nm": item.max_skew_nm,
        "status": item.status,
        "skew_before_nm": item.skew_before_nm,
        "skew_after_nm": item.skew_after_nm,
        "members": [
            {"net": member.net, "length_before_nm": member.length_before_nm,
             "length_after_nm": member.length_after_nm, "added_length_nm": member.added_length_nm}
            for member in item.members
        ],
        "bumps": item.bumps,
        "reason": item.reason,
    }


def match_tuning_line(item: MatchTuningResult) -> str:
    """One console line per group, as printed by the critical preflight."""
    def mm(value: int | None) -> str:
        return "n/a" if value is None else f"{value / 1e6:.3f}"

    added = ", ".join(f"{member.net} +{mm(member.added_length_nm)}"
                      for member in item.members if member.added_length_nm)
    return (f"match group {item.id}: {item.status}, skew {mm(item.skew_before_nm)} -> "
            f"{mm(item.skew_after_nm)} mm (max {mm(item.max_skew_nm)} mm)"
            + (f"; added {added} mm" if added else "")
            + (f"; {item.bumps} bumps" if item.bumps else "")
            + (f": {item.reason}" if item.reason else ""))


@dataclass(frozen=True, slots=True)
class _Slot:
    run: tuple[int, int]        # first member's track index, partner's (-1 for one net)
    position: int               # grid index inside the run
    layer: CopperLayer
    horizontal: bool            # the run is along x
    side: int                   # +1 or -1 along the normal axis
    outer: int                  # track index of the member on the bulge side
    inner: int | None           # track index of the other pair member
    base: int                   # normal coordinate of the outer member's line
    start: int                  # outer member's bump interval along the axis
    end: int
    spacing: int                # lane distance of a pair, 0 for a single net
    width: int


class UnitTuner:
    """Candidate bump slots of one unit, with their room, in placement order.

    ``tracks`` is the unit's accepted copper and ``obstacles`` indexes every
    other copper object. ``nets`` has one net, or both pair members, whose
    lanes are ``spacing_nm`` apart. Rooms are measured once, so several
    length targets cost one survey.
    """

    def __init__(self, board: PhysicalBoard, obstacles: RoutingClearanceIndex,
                 tracks: tuple[TrackSegment, ...], nets: tuple[str, ...], spacing_nm: int,
                 amplitude_limit_nm: int, clearance_nm: int,
                 bump_limit: int = MATCH_TUNING_BUMP_LIMIT) -> None:
        self.tracks = tracks
        self.clearance_nm = clearance_nm
        self.bump_limit = bump_limit
        slots = _slots(tracks, nets, spacing_nm) if amplitude_limit_nm > 0 else []
        self.rooms = {slot: _room(board, obstacles, nets, slot, amplitude_limit_nm) for slot in slots}
        ends = _terminal_ends(tracks)

        def remoteness(slot: _Slot) -> int:
            middle = (slot.start + slot.end) // 2
            point = Point(middle, slot.base) if slot.horizontal else Point(slot.base, middle)
            return min(((point.x_nm - end.x_nm) ** 2 + (point.y_nm - end.y_nm) ** 2 for end in ends),
                       default=0)

        self.ordered = sorted((slot for slot in slots if self.rooms[slot] > 0), key=lambda slot: (
            -self.rooms[slot], -remoteness(slot), slot.layer.value, not slot.horizontal, slot.base,
            slot.start, slot.side))

    def tune(self, added_nm: int) -> tuple[tuple[TrackSegment, ...] | None, int, str]:
        """Add ``added_nm`` (even) to every net of the unit.

        A bump adds exactly ``bump_gain`` to each member. Returns the tuned
        copper in the original track order (each bumped segment is replaced
        in place), the bump count, and the reason with ``None`` copper when
        the length does not fit.
        """
        if added_nm <= 0:
            return None, 0, "no length to add"
        chosen: list[_Slot] = []
        capacity = 0
        for slot in self.ordered:
            if capacity >= added_nm or len(chosen) >= self.bump_limit:
                break
            if any(_conflict(slot, other, self.rooms[slot], self.rooms[other], self.clearance_nm)
                   for other in chosen):
                continue
            chosen.append(slot)
            capacity += _slot_gain(slot, self.rooms[slot])
        if capacity < added_nm:
            limit = f", the limit of {self.bump_limit} bumps" if len(chosen) >= self.bump_limit else ""
            return None, 0, (f"insufficient tuning room: {capacity} of {added_nm} nm "
                             f"in {len(chosen)} bump(s){limit}")
        heights = _levelled([self.rooms[slot] for slot in chosen], added_nm,
                            [lambda height, slot=slot: _slot_gain(slot, height) for slot in chosen])
        bumps: dict[int, list[tuple[int, int, int, int, int, int]]] = {}
        count = 0
        for slot, height in zip(chosen, heights):
            if not _slot_gain(slot, height):
                continue
            count += 1
            # The bulge-side member is inside the turn at the run corners, the
            # other member at the top corners.
            inside, outside = bump_chamfers(height, slot.width, slot.spacing)
            bumps.setdefault(slot.outer, []).append((slot.start, slot.end, slot.side, height, inside, outside))
            if slot.inner is not None:
                bumps.setdefault(slot.inner, []).append(
                    (slot.start + slot.spacing, slot.end - slot.spacing, slot.side, height, outside, inside))
        tuned: list[TrackSegment] = []
        for index, track in enumerate(self.tracks):
            tuned.extend(_bumped(track, bumps[index]) if index in bumps else (track,))
        return tuple(tuned), count, ""


def bump_chamfers(height: int, width: int, spacing: int = 0) -> tuple[int, int]:
    """Chamfer legs ``(inside, outside)`` of a bump's corners (plan R9).

    At each corner the member inside the turn takes ``inside`` and its pair
    partner ``outside``, ``inside + d`` with d = floor((2 - sqrt 2) x
    ``spacing``) (0 for a single net), so the 45-degree pieces of both lanes
    are at least ``spacing`` apart. ``inside`` is the track ``width``, at most
    a quarter of ``height``, and small enough to leave ``height - inside -
    outside >= 0`` of straight leg. A bump too low for ``inside >= 1``
    (``height < 4`` or ``height < d + 2``) has one 45-degree ramp per side:
    ``inside + outside == height`` and a leg below 1 moves the ramp's foot
    into the bump.
    """
    offset = 2 * spacing - isqrt(2 * spacing * spacing) - 1 if spacing > 0 else 0
    inside = min(width, height // 4, (height - offset) // 2)
    if inside < 1:
        inside = -((offset - height) // 2)
        return inside, height - inside
    return inside, inside + offset


def bump_gain(height: int, base: int, top: int) -> int:
    """Length a member gains from a bump, as critical lengths measure it.

    ``base``/``top`` are the member's chamfer legs at the run and top corners
    (``bump_chamfers``). Every piece of ``bump_path`` is axis-aligned or exactly
    45 degrees and its length is rounded on its own.
    """
    def diagonal(leg: int) -> int:
        return round(hypot(leg, leg))

    if min(base, top) < 1:
        return 2 * (diagonal(height) - height)
    return 2 * height + 2 * (diagonal(base) + diagonal(top)) - 4 * (base + top)


def bump_path(entry: int, leave: int, height: int, base: int, top: int) -> tuple[tuple[int, int], ...]:
    """Centre-line corners ``(axial, rise)`` of one member's bump, in travel order.

    ``entry``/``leave`` are the axial positions of the legs; ``base``/``top``
    are the chamfer legs at the run and top corners (``bump_chamfers``). A leg
    below 1 makes each side one ramp; otherwise the two chamfers of a leg stay
    separate pieces even when no straight leg is left between them.
    """
    sign = 1 if leave > entry else -1
    if min(base, top) < 1:
        return ((entry - sign * base, 0), (entry + sign * top, height),
                (leave - sign * top, height), (leave + sign * base, 0))
    return ((entry - sign * base, 0), (entry, base), (entry, height - top), (entry + sign * top, height),
            (leave - sign * top, height), (leave, height - top), (leave, base), (leave + sign * base, 0))


def _slot_gain(slot: _Slot, height: int) -> int:
    """Length each member of the unit gains from a bump of ``height`` in ``slot``."""
    return bump_gain(height, *bump_chamfers(height, slot.width, slot.spacing))


def _horizontal(track: TrackSegment) -> bool | None:
    """True along x, False along y, None for a diagonal segment."""
    if track.start.y_nm == track.end.y_nm:
        return True
    if track.start.x_nm == track.end.x_nm:
        return False
    return None


def _slots(tracks: tuple[TrackSegment, ...], nets: tuple[str, ...], spacing_nm: int) -> list[_Slot]:
    """Every candidate slot of the unit, on both sides of each straight run."""
    def axial(point: Point, horizontal: bool) -> int:
        return point.x_nm if horizontal else point.y_nm

    def normal(point: Point, horizontal: bool) -> int:
        return point.y_nm if horizontal else point.x_nm

    runs: list[tuple[int, int, bool, int, int, int]] = []
    for first, track in enumerate(tracks):
        horizontal = _horizontal(track)
        if horizontal is None or track.net != nets[0]:
            continue
        low, high = sorted((axial(track.start, horizontal), axial(track.end, horizontal)))
        if len(nets) == 1:
            runs.append((first, -1, horizontal, low, high, 0))
            continue
        for second, partner in enumerate(tracks):
            if (partner.net != nets[1] or partner.layer is not track.layer
                    or _horizontal(partner) is not horizontal):
                continue
            distance = abs(normal(track.start, horizontal) - normal(partner.start, horizontal))
            if abs(distance - spacing_nm) > _LANE_TOLERANCE_NM:
                continue
            other_low, other_high = sorted((axial(partner.start, horizontal), axial(partner.end, horizontal)))
            if max(low, other_low) < min(high, other_high):
                runs.append((first, second, horizontal, max(low, other_low), min(high, other_high), distance))
    slots: list[_Slot] = []
    for first, second, horizontal, low, high, distance in runs:
        width = max(tracks[first].width_nm, tracks[second].width_nm if second >= 0 else 0)
        pitch = 3 * width
        bump = pitch + 2 * distance
        usable = high - low - 2 * pitch
        count = (usable + pitch) // (bump + pitch) if usable >= bump else 0
        offset = low + pitch + (usable - count * bump - (count - 1) * pitch) // 2
        for position in range(count):
            start = offset + position * (bump + pitch)
            for side in (1, -1):
                outer, inner = first, None
                if second >= 0:
                    lane = normal(tracks[first].start, horizontal) - normal(tracks[second].start, horizontal)
                    outer, inner = (first, second) if side * lane > 0 else (second, first)
                slots.append(_Slot((first, second), position, tracks[first].layer, horizontal, side,
                                   outer, inner, normal(tracks[outer].start, horizontal),
                                   start, start + bump, distance, width))
    return slots


def _swept(slot: _Slot, height: int) -> RoundedConvexShape:
    """Area covered by a bump's copper (every member) of ``height``.

    It is the convex outline of the bulge-side member's chamfered bump: its
    run chamfers are bridged by the straight edge from the foot to the leg's
    top end. The other member of a pair lies inside it, except where its run
    chamfers dip between the two lanes.
    """
    corners = bump_path(slot.start, slot.end, height, *bump_chamfers(height, slot.width, slot.spacing))
    if len(corners) == 8:
        corners = corners[:1] + corners[2:6] + corners[7:]

    def point(axial: int, rise: int) -> Point:
        normal = slot.base + slot.side * rise
        return Point(axial, normal) if slot.horizontal else Point(normal, axial)

    return RoundedConvexShape(tuple(point(*corner) for corner in corners), slot.width // 2)


def _room(board: PhysicalBoard, obstacles: RoutingClearanceIndex, nets: tuple[str, ...],
          slot: _Slot, limit: int) -> int:
    """Tallest clear bump height in ``0..limit`` (the swept area only grows)."""
    def clear(height: int) -> bool:
        shape = _swept(slot, height)
        # Each member is checked as its own net, so the other member's lands
        # count as foreign copper too.
        return (shape_in_board(board, shape, board.rules.minimum_clearance_nm,
                               board.rules.minimum_hole_clearance_nm)
                and all(obstacles.can_area(net, shape, slot.layer) for net in nets))

    if not clear(1):
        return 0
    if clear(limit):
        return limit
    low, high = 1, limit
    while high - low > 1:
        middle = (low + high) // 2
        low, high = (middle, high) if clear(middle) else (low, middle)
    return low


def _conflict(slot: _Slot, other: _Slot, room: int, other_room: int, clearance_nm: int) -> bool:
    """Two slots cannot both be used.

    Slots of one run are spaced by construction; only one side of each grid
    position can be used. Slots of different runs must keep their swept areas
    (at full room) apart by the clearance.
    """
    if slot.run == other.run:
        return slot.position == other.position
    if slot.layer is not other.layer:
        return False
    margin = slot.width // 2 + other.width // 2 + clearance_nm
    first, second = _swept(slot, room).bounds, _swept(other, other_room).bounds
    return Bounds(first.min_x - margin, first.min_y - margin,
                  first.max_x + margin, first.max_y + margin).intersects(second)


def _terminal_ends(tracks: tuple[TrackSegment, ...]) -> tuple[Point, ...]:
    """Track endpoints used once per net: the unit's terminals and stubs."""
    counts: dict[tuple[str, Point], int] = {}
    for track in tracks:
        for point in (track.start, track.end):
            counts[(track.net, point)] = counts.get((track.net, point), 0) + 1
    return tuple(sorted({point for (_, point), count in counts.items() if count == 1},
                        key=lambda point: (point.x_nm, point.y_nm)))


def _levelled(rooms: list[int], total: int, gains: list[Callable[[int], int]]) -> list[int]:
    """Heights within each room whose gains sum to ``total``, as level as possible.

    ``gains[i]`` maps a height to the length bump ``i`` adds; one nanometre
    more height changes it by -2, 0 or +2 (a larger chamfer can cost more than
    the leg adds). The lowest common level ``L`` whose gains reach ``total`` is
    used; the excess is taken from the last slots at that level whose gain
    drops by 2 one nanometre lower. Since the gains at ``L - 1`` fall short,
    those slots suffice.
    """
    def reached(level: int) -> int:
        return sum(gain(min(room, level)) for room, gain in zip(rooms, gains))

    low, high = 0, max(rooms)   # below the total at ``low``, reaching it at ``high``
    while high - low > 1:
        middle = (low + high) // 2
        low, high = (middle, high) if reached(middle) < total else (low, middle)
    level = high
    heights = [min(room, level) for room in rooms]
    excess = reached(level) - total
    for index in reversed(range(len(heights))):
        if excess < 2:
            break
        if heights[index] == level and gains[index](level) - gains[index](level - 1) == 2:
            heights[index] -= 1
            excess -= 2
    return heights


def _bumped(track: TrackSegment, bumps: list[tuple[int, int, int, int, int, int]]) -> list[TrackSegment]:
    """Replace one axis-aligned segment with chamfered bumps.

    Each bump is ``(start, end, side, height, base, top)`` with ``start``/
    ``end`` the legs along the segment's axis and ``base``/``top`` the chamfer
    legs at the run and top corners; it rises ``height`` toward ``side`` of the
    segment's own line, so the inner member of a pair bends at its own lane.
    """
    horizontal = _horizontal(track)
    assert horizontal is not None
    axial = (lambda point: point.x_nm) if horizontal else (lambda point: point.y_nm)
    normal = track.start.y_nm if horizontal else track.start.x_nm
    sign = 1 if axial(track.end) > axial(track.start) else -1

    def point(along: int, offset: int) -> Point:
        return Point(along, normal + offset) if horizontal else Point(normal + offset, along)

    points = [track.start]
    for start, end, side, height, base, top in sorted(bumps, key=lambda bump: sign * bump[0]):
        entry, leave = (start, end) if sign > 0 else (end, start)
        points.extend(point(along, side * rise) for along, rise in bump_path(entry, leave, height, base, top))
    points.append(track.end)
    return [TrackSegment(track.net, start, end, track.width_nm, track.layer)
            for start, end in zip(points, points[1:]) if start != end]
