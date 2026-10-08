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

S-shaped serpentines (plan R10). A unit whose rules declare ``tuning_style =
"serpentine"`` snakes about the original line instead. Its collinear pieces
are joined into straight lines first (``_straight_lines``; for example a line
cut at a breakout-region boundary), and on a straight run the legs cross the
line perpendicular to it at a pitch of the lane width (both
members and their gap) plus the leg gap (``tuning_spacing``, by default the
larger of 3 x width and the clearance), and the *tops* between them alternate
sides. A top of height a adds 2a, half to each adjacent leg. Each leg has
one corner at each end: every member is inside one and outside the other, so
with one chamfer leg c for the whole unit (R9's rule applied to every leg, and
small enough to leave a straight piece on every top) each leg costs both
members the same, and a serpentine of n tops adds exactly 2 x (sum of heights)
- (n + 1)(f(c) + f(c + d)), f(k) = 2k - round(k sqrt 2). A top's room is that
of a bump over its two legs on its side, with the outline taken at the least
top and largest foot chamfer, so it holds the copper for any c. A run takes
one serpentine (a window of at least two consecutive tops with room on their
sides); windows with the most capacity come first, the last one trimmed to
the fewest tops. Runs without one keep R3's one-sided bumps, and a unit
whose serpentines do not provide the length falls back to bumps alone. At most
``MATCH_TUNING_LEG_LIMIT`` legs are used (a bump has two); heights are
levelled across tops and bumps, and c is lowered until it suits every leg
and makes the length exact. Bump-style units keep one run per piece, so they
tune exactly as before.

This module holds the per-group report and the bump and serpentine geometry.
The critical router owns the order, the length targets and the atomic
validation (``critical.py``).
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from math import hypot, isqrt
from typing import Callable

from .geometry import Bounds, RoundedConvexShape
from .mechanical import shape_in_board
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment, TuningStyle, Via
from .routing_clearance import RoutingClearanceIndex


# Bound on serpentine bumps per tuned unit.
MATCH_TUNING_BUMP_LIMIT = 16

# Bound on serpentine legs per tuned unit (plan R10): as many as 16 bumps have.
MATCH_TUNING_LEG_LIMIT = 2 * MATCH_TUNING_BUMP_LIMIT

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
class MatchTuningUnit:
    """One tuning step of one unit (plan R10/R12).

    ``stage`` is ``routing`` for a bundle pair tuned as soon as it was
    accepted (R12) and ``final`` for the pass after every group (R3).
    ``style`` is ``serpentine`` when the step used S-shaped legs, else
    ``bumps``; ``amplitudes_nm`` lists every top's and bump's height in
    placement order. A step of a tuning group (plan R14) names the ``group``,
    has every unit's nets in ``nets`` and its ``lanes`` across the run where it
    bent them, in order.
    """

    nets: tuple[str, ...]
    stage: str
    style: str
    added_length_nm: int
    bumps: int
    legs: int
    amplitudes_nm: tuple[int, ...]
    group: str = ""
    lanes: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class TuningGroupOutcome:
    """Outcome of one tuning group of a ``length_match`` group (plan R14).

    ``status`` is ``tuned`` (a group step's copper was kept), ``failed`` (no
    group step was kept; ``reason`` says why), ``within_limit`` (no unit of
    the group needed length) or ``incomplete`` (a unit is not connected).
    """

    name: str
    nets: tuple[str, ...]
    status: str
    reason: str = ""


@dataclass(frozen=True, slots=True)
class MatchTuningResult:
    """Outcome of tuning one ``length_match`` group.

    ``status`` is ``within_limit`` (copper untouched), ``tuned``, ``failed``
    (copper kept exactly as routed; ``reason`` says why) or ``incomplete`` (a
    member is not connected after critical routing, so the group is not judged
    here). With tuning while routing (R12), lengths and skew *before* are
    those of the copper as routed, before any tuning, and a failed group keeps
    the copper tuned while routing. ``units`` lists every tuning step; it is
    reported only when a step used a serpentine, ran while routing or tuned a
    tuning group. ``tuning_groups`` gives each declared tuning group's outcome.
    """

    id: str
    max_skew_nm: int
    status: str
    skew_before_nm: int | None
    skew_after_nm: int | None
    members: tuple[MatchTuningMember, ...]
    bumps: int = 0
    reason: str = ""
    units: tuple[MatchTuningUnit, ...] = ()
    tuning_groups: tuple[TuningGroupOutcome, ...] = ()

    @property
    def legs(self) -> int:
        return sum(unit.legs for unit in self.units)

    @property
    def reported_units(self) -> bool:
        return any(unit.stage == "routing" or unit.legs or unit.group for unit in self.units)


def match_tuning_document(item: MatchTuningResult) -> dict[str, object]:
    """JSON-ready form of one group's tuning outcome for the critical report."""
    document: dict[str, object] = {
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
    if item.reported_units:
        document["legs"] = item.legs
        document["units"] = [
            {"nets": list(unit.nets), "stage": unit.stage, "style": unit.style,
             "added_length_nm": unit.added_length_nm, "bumps": unit.bumps, "legs": unit.legs,
             "amplitudes_nm": list(unit.amplitudes_nm),
             **({"group": unit.group, "lanes": list(unit.lanes)} if unit.group else {})}
            for unit in item.units
        ]
    if item.tuning_groups:
        document["tuning_groups"] = [
            {"name": group.name, "nets": list(group.nets), "status": group.status, "reason": group.reason}
            for group in item.tuning_groups
        ]
    return document


def match_tuning_line(item: MatchTuningResult) -> str:
    """One console line per group, as printed by the critical preflight."""
    def mm(value: int | None) -> str:
        return "n/a" if value is None else f"{value / 1e6:.3f}"

    added = ", ".join(f"{member.net} +{mm(member.added_length_nm)}"
                      for member in item.members if member.added_length_nm)
    steps = ""
    if item.reported_units:
        steps = "; " + ", ".join(
            (f"group {unit.group} {'/'.join(unit.lanes)}" if unit.group else "/".join(unit.nets))
            + f" {unit.style}"
            + (f" {unit.legs} legs" if unit.legs else "") + (f" {unit.bumps} bumps" if unit.bumps else "")
            + f" up to {mm(max(unit.amplitudes_nm, default=0))} mm"
            + (" while routing" if unit.stage == "routing" else "")
            for unit in item.units)
    groups = "".join(f"; tuning group {group.name} {group.status}" + (f" ({group.reason})" if group.reason else "")
                     for group in item.tuning_groups if group.status != "tuned")
    return (f"match group {item.id}: {item.status}, skew {mm(item.skew_before_nm)} -> "
            f"{mm(item.skew_after_nm)} mm (max {mm(item.max_skew_nm)} mm)"
            + (f"; added {added} mm" if added else "")
            + (f"; {item.bumps} bumps" if item.bumps and not steps else "")
            + steps + groups
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


@dataclass(frozen=True, slots=True)
class UnitTuning:
    """One unit's tuning: the tuned copper, or ``None`` and the reason.

    ``legs`` counts serpentine legs (plan R10) and ``amplitudes_nm`` every
    serpentine top's and bump's height, in placement order.
    """

    tracks: tuple[TrackSegment, ...] | None
    bumps: int = 0
    legs: int = 0
    amplitudes_nm: tuple[int, ...] = ()
    reason: str = ""
    # A tuning group's lanes across its run, in order (plan R14).
    lanes: tuple[str, ...] = ()

    @property
    def style(self) -> str:
        return "serpentine" if self.legs else "bumps"


@dataclass(frozen=True, slots=True)
class _Window:
    """Consecutive serpentine tops of one run on alternating sides (plan R10)."""

    tops: tuple[_Slot, ...]     # each top as the bump over its two legs on its side
    rooms: tuple[int, ...]
    pitch: int                  # leg pitch along the run
    gap: int                    # least edge gap between adjacent legs


class UnitTuner:
    """Candidate bump slots of one unit, with their room, in placement order.

    ``tracks`` is the unit's accepted copper and ``obstacles`` indexes every
    other copper object. ``nets`` has one net, or both pair members, whose
    lanes are ``spacing_nm`` apart. Rooms are measured once, so several
    length targets cost one survey. With ``style`` serpentine, the serpentine
    windows of every run are surveyed too; ``leg_gap_nm`` is the declared
    ``tuning_spacing`` (None for the default).
    """

    def __init__(self, board: PhysicalBoard, obstacles: RoutingClearanceIndex,
                 tracks: tuple[TrackSegment, ...], nets: tuple[str, ...], spacing_nm: int,
                 amplitude_limit_nm: int, clearance_nm: int,
                 bump_limit: int = MATCH_TUNING_BUMP_LIMIT, style: TuningStyle = TuningStyle.BUMPS,
                 leg_gap_nm: int | None = None, leg_limit: int = MATCH_TUNING_LEG_LIMIT,
                 vias: tuple[Via, ...] = ()) -> None:
        self.tracks = tracks
        self.clearance_nm = clearance_nm
        self.bump_limit = bump_limit
        self.leg_limit = leg_limit
        serpentine = TuningStyle(style) is TuningStyle.SERPENTINE
        # Straight lines by their first piece's index: each piece on its own,
        # or (serpentines) collinear pieces joined.
        self.segments: dict[int, TrackSegment] = dict(enumerate(tracks))
        self.lead_of: dict[int, int] = {}
        if serpentine:
            lines = _straight_lines(tracks, vias)
            self.segments = {lead: line for lead, (_, line) in lines.items()}
            self.lead_of = {index: lead for lead, (indices, _) in lines.items() for index in indices}
        runs = _runs(self.segments, nets, spacing_nm) if amplitude_limit_nm > 0 else []
        slots = _slots(self.segments, runs)
        self.rooms = {slot: _room(board, obstacles, nets, slot, amplitude_limit_nm) for slot in slots}
        ends = _terminal_ends(tracks)

        def remoteness(slot: _Slot) -> int:
            middle = (slot.start + slot.end) // 2
            point = Point(middle, slot.base) if slot.horizontal else Point(slot.base, middle)
            return min(((point.x_nm - end.x_nm) ** 2 + (point.y_nm - end.y_nm) ** 2 for end in ends),
                       default=0)

        self.remoteness = remoteness
        self.ordered = sorted((slot for slot in slots if self.rooms[slot] > 0), key=lambda slot: (
            -self.rooms[slot], -remoteness(slot), slot.layer.value, not slot.horizontal, slot.base,
            slot.start, slot.side))
        self.windows: list[_Window] = []
        if serpentine:
            for run in runs:
                self.windows.extend(_windows(board, obstacles, self.segments, nets, run, amplitude_limit_nm,
                                             leg_gap_nm, clearance_nm))
            self.windows.sort(key=lambda window: (
                -_window_capacity(window, window.rooms), -remoteness(_span(window)),
                window.tops[0].layer.value, not window.tops[0].horizontal, window.tops[0].base,
                window.tops[0].start, window.tops[0].side))

    def tune(self, added_nm: int) -> tuple[tuple[TrackSegment, ...] | None, int, str]:
        """Add ``added_nm`` (even) to every net of the unit.

        A bump adds exactly ``bump_gain`` to each member. Returns the tuned
        copper in the original track order (each bumped segment is replaced
        in place), the bump count, and the reason with ``None`` copper when
        the length does not fit. ``plan`` also reports serpentines.
        """
        result = self.plan(added_nm)
        return result.tracks, result.bumps, result.reason

    def plan(self, added_nm: int) -> UnitTuning:
        """Add ``added_nm`` to every net of the unit: serpentines first, else bumps alone."""
        if added_nm <= 0:
            return UnitTuning(None, reason="no length to add")
        if not self.windows:
            return self._bumps(added_nm)
        serpentine = self._serpentines(added_nm)
        if serpentine.tracks is not None:
            return serpentine
        bumps = self._bumps(added_nm)
        if bumps.tracks is None:
            return replace(bumps, reason=f"{bumps.reason}; serpentine: {serpentine.reason}")
        return bumps

    def _bumps(self, added_nm: int) -> UnitTuning:
        """One-sided bumps only (plan R3), most room first."""
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
            return UnitTuning(None, reason=f"insufficient tuning room: {capacity} of {added_nm} nm "
                                           f"in {len(chosen)} bump(s){limit}")
        heights = _levelled([self.rooms[slot] for slot in chosen], added_nm,
                            [lambda height, slot=slot: _slot_gain(slot, height) for slot in chosen])
        features: dict[int, list[list[tuple[int, int]]]] = {}
        used = _add_bumps(features, chosen, heights)
        return UnitTuning(self._spliced(features), len(used), 0, tuple(used))

    def _serpentines(self, added_nm: int) -> UnitTuning:
        """Serpentine windows, most capacity first, then bumps on the other runs."""
        chosen: list[_Window] = []
        legs = capacity = 0
        for window in self.windows:
            if capacity >= added_nm or legs + 3 > self.leg_limit:
                break
            if any(window.tops[0].run == other.tops[0].run
                   or _near(_window_bounds(window), _window_bounds(other),
                            window.tops[0].width // 2 + other.tops[0].width // 2 + self.clearance_nm)
                   for other in chosen):
                continue
            window = self._trimmed(window, added_nm - capacity, self.leg_limit - legs)
            chosen.append(window)
            legs += len(window.tops) + 1
            capacity += _window_capacity(window, window.rooms)
        bumps: list[_Slot] = []
        runs = {window.tops[0].run for window in chosen}
        for slot in self.ordered:
            if not chosen or capacity >= added_nm or legs + 2 > self.leg_limit:
                break
            bounds = _swept(slot, self.rooms[slot]).bounds
            if (slot.run in runs
                    or any(_near(bounds, _window_bounds(window),
                                 slot.width // 2 + window.tops[0].width // 2 + self.clearance_nm)
                           for window in chosen)
                    or any(_conflict(slot, other, self.rooms[slot], self.rooms[other], self.clearance_nm)
                           for other in bumps)):
                continue
            bumps.append(slot)
            legs += 2
            capacity += _slot_gain(slot, self.rooms[slot])
        if not chosen or capacity < added_nm:
            limit = f", the limit of {self.leg_limit} legs" if legs + 2 > self.leg_limit else ""
            return UnitTuning(None, reason=f"insufficient tuning room: {capacity} of {added_nm} nm "
                                           f"in {legs} leg(s){limit}")
        # One chamfer leg for every serpentine, lowered until it suits every
        # leg of the levelled heights and makes the added length even.
        rooms = [room for window in chosen for room in window.rooms] + [self.rooms[slot] for slot in bumps]
        gains = [_double] * (len(rooms) - len(bumps)) + [
            lambda height, slot=slot: _slot_gain(slot, height) for slot in bumps]
        chamfer, heights = min(_window_chamfer(window, window.rooms) for window in chosen), rooms
        for _ in range(_CHAMFER_STEPS):
            if chamfer < 1:
                break
            total = added_nm + sum((len(window.tops) + 1) * _leg_loss(chamfer, window.tops[0].spacing)
                                   for window in chosen)
            if total % 2:
                chamfer -= 1
                continue
            heights = _levelled(rooms, total, gains)
            fitting = min(_window_chamfer(window, part) for window, part in zip(chosen, _parts(chosen, heights)))
            if fitting >= chamfer:
                break
            chamfer = fitting
        else:
            chamfer = 0
        if chamfer < 1:
            return UnitTuning(None, reason="serpentine tops too low for 45-degree corners")
        features: dict[int, list[list[tuple[int, int]]]] = {}
        amplitudes: list[int] = []
        for window, part in zip(chosen, _parts(chosen, heights)):
            _add_serpentine(features, self.segments, window, part, chamfer)
            amplitudes.extend(part)
        used = _add_bumps(features, bumps, heights[len(heights) - len(bumps):])
        return UnitTuning(self._spliced(features), len(used), sum(len(window.tops) + 1 for window in chosen),
                          (*amplitudes, *used))

    def _trimmed(self, window: _Window, need: int, legs: int) -> _Window:
        """The fewest consecutive tops of ``window`` that add ``need``, within ``legs`` legs.

        Of the windows of that length, the one with the most capacity is kept,
        then the one farthest from the terminals, then the first.
        """
        most = min(len(window.tops), legs - 1)
        for count in range(2, most + 1):
            first = max(range(len(window.tops) - count + 1), key=lambda first: (
                _window_capacity(window, window.rooms[first:first + count]),
                self.remoteness(_span(_cut(window, first, count))), -first))
            part = _cut(window, first, count)
            if count == most or _window_capacity(part, part.rooms) >= need:
                return part
        return window

    def _spliced(self, features: dict[int, list[list[tuple[int, int]]]]) -> tuple[TrackSegment, ...]:
        """The unit's copper in the original track order, each tuned line replacing its pieces."""
        tuned: list[TrackSegment] = []
        for index, track in enumerate(self.tracks):
            lead = self.lead_of.get(index, index)
            if lead not in features:
                tuned.append(track)
            elif index == lead:
                tuned.extend(_bumped(self.segments[lead], features[lead]))
        return tuple(tuned)


# Bound on the chamfer steps of one serpentine plan; each step lowers the
# chamfer leg, and a few usually suffice.
_CHAMFER_STEPS = 64


def _double(height: int) -> int:
    return 2 * height


def _add_bumps(features: dict[int, list[list[tuple[int, int]]]], slots: list[_Slot],
               heights: list[int]) -> list[int]:
    """Add each bump's corners to its members; the heights of the bumps that add length."""
    used = []
    for slot, height in zip(slots, heights):
        if not _slot_gain(slot, height):
            continue
        used.append(height)
        # The bulge-side member is inside the turn at the run corners, the
        # other member at the top corners.
        inside, outside = bump_chamfers(height, slot.width, slot.spacing)
        features.setdefault(slot.outer, []).append(
            [(along, slot.side * rise) for along, rise in bump_path(slot.start, slot.end, height, inside, outside)])
        if slot.inner is not None:
            features.setdefault(slot.inner, []).append(
                [(along, slot.side * rise) for along, rise in bump_path(
                    slot.start + slot.spacing, slot.end - slot.spacing, height, outside, inside)])
    return used


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
    offset = _chamfer_offset(spacing)
    inside = min(width, height // 4, (height - offset) // 2)
    if inside < 1:
        inside = -((offset - height) // 2)
        return inside, height - inside
    return inside, inside + offset


def _chamfer_offset(spacing: int) -> int:
    """d = floor((2 - sqrt 2) x ``spacing``), 0 for a single net (spacing 0)."""
    return 2 * spacing - isqrt(2 * spacing * spacing) - 1 if spacing > 0 else 0


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


def _axial(point: Point, horizontal: bool) -> int:
    return point.x_nm if horizontal else point.y_nm


def _normal(point: Point, horizontal: bool) -> int:
    return point.y_nm if horizontal else point.x_nm


_Run = tuple[int, int, bool, int, int, int]


def _straight_lines(tracks: tuple[TrackSegment, ...],
                    vias: tuple[Via, ...]) -> dict[int, tuple[tuple[int, ...], TrackSegment]]:
    """Maximal straight lines of the unit's axis-aligned pieces, by their first index.

    Collinear pieces of one net, layer and width join where they meet end to
    end at a point no other piece or via of the net touches (for example
    where a line was cut at a breakout-region boundary). Each line is one
    segment in its first piece's direction.
    """
    ends: dict[tuple[str, CopperLayer, Point], int] = {}
    for track in tracks:
        for point in (track.start, track.end):
            ends[(track.net, track.layer, point)] = ends.get((track.net, track.layer, point), 0) + 1
    blocked = {(via.net, via.position) for via in vias}

    def key(index: int) -> tuple:
        track = tracks[index]
        horizontal = _horizontal(track)
        return (track.net, track.layer.value, horizontal, _normal(track.start, horizontal),
                min(_axial(track.start, horizontal), _axial(track.end, horizontal)), index)

    chains: list[list[int]] = []
    for index in sorted((index for index, track in enumerate(tracks)
                         if track.start != track.end and _horizontal(track) is not None), key=key):
        track, horizontal = tracks[index], _horizontal(tracks[index])
        if chains:
            last = tracks[chains[-1][-1]]
            joint = min(track.start, track.end, key=lambda point: _axial(point, horizontal))
            if (last.net == track.net and last.layer is track.layer and _horizontal(last) is horizontal
                    and last.width_nm == track.width_nm
                    and joint == max(last.start, last.end, key=lambda point: _axial(point, horizontal))
                    and _normal(last.start, horizontal) == _normal(track.start, horizontal)
                    and ends[(track.net, track.layer, joint)] == 2 and (track.net, joint) not in blocked):
                chains[-1].append(index)
                continue
        chains.append([index])
    lines = {}
    for chain in chains:
        lead = min(chain)
        horizontal = _horizontal(tracks[lead])
        points = sorted({point for index in chain for point in (tracks[index].start, tracks[index].end)},
                        key=lambda point: _axial(point, horizontal))
        first, last = points[0], points[-1]
        if _axial(tracks[lead].end, horizontal) < _axial(tracks[lead].start, horizontal):
            first, last = last, first
        lines[lead] = (tuple(sorted(chain)), tracks[lead] if len(chain) == 1 else replace(
            tracks[lead], start=first, end=last))
    return dict(sorted(lines.items()))


def _runs(segments: dict[int, TrackSegment], nets: tuple[str, ...], spacing_nm: int) -> list[_Run]:
    """Straight runs of the unit: ``(first, second, horizontal, low, high, distance)``.

    For a pair, ``first``/``second`` key the parallel ``segments`` of both
    members on one layer, ``spacing_nm`` apart, over their common interval;
    for a single net every axis-aligned segment, with ``second`` -1.
    """
    runs: list[_Run] = []
    for first, track in segments.items():
        horizontal = _horizontal(track)
        if horizontal is None or track.net != nets[0]:
            continue
        low, high = sorted((_axial(track.start, horizontal), _axial(track.end, horizontal)))
        if len(nets) == 1:
            runs.append((first, -1, horizontal, low, high, 0))
            continue
        for second, partner in segments.items():
            if (partner.net != nets[1] or partner.layer is not track.layer
                    or _horizontal(partner) is not horizontal):
                continue
            distance = abs(_normal(track.start, horizontal) - _normal(partner.start, horizontal))
            if abs(distance - spacing_nm) > _LANE_TOLERANCE_NM:
                continue
            other_low, other_high = sorted((_axial(partner.start, horizontal), _axial(partner.end, horizontal)))
            if max(low, other_low) < min(high, other_high):
                runs.append((first, second, horizontal, max(low, other_low), min(high, other_high), distance))
    return runs


def _run_slot(tracks: dict[int, TrackSegment], run: _Run, position: int, side: int,
              start: int, end: int, width: int) -> _Slot:
    """The bump of ``run`` over the outer member's interval ``start..end`` toward ``side``."""
    first, second, horizontal, _, _, distance = run
    outer, inner = first, None
    if second >= 0:
        lane = _normal(tracks[first].start, horizontal) - _normal(tracks[second].start, horizontal)
        outer, inner = (first, second) if side * lane > 0 else (second, first)
    return _Slot((first, second), position, tracks[first].layer, horizontal, side, outer, inner,
                 _normal(tracks[outer].start, horizontal), start, end, distance, width)


def _slots(tracks: dict[int, TrackSegment], runs: list[_Run]) -> list[_Slot]:
    """Every candidate bump slot of the unit, on both sides of each straight run."""
    slots: list[_Slot] = []
    for run in runs:
        first, second, _, low, high, distance = run
        width = max(tracks[first].width_nm, tracks[second].width_nm if second >= 0 else 0)
        pitch = 3 * width
        bump = pitch + 2 * distance
        usable = high - low - 2 * pitch
        count = (usable + pitch) // (bump + pitch) if usable >= bump else 0
        offset = low + pitch + (usable - count * bump - (count - 1) * pitch) // 2
        for position in range(count):
            start = offset + position * (bump + pitch)
            for side in (1, -1):
                slots.append(_run_slot(tracks, run, position, side, start, start + bump, width))
    return slots


def _windows(board: PhysicalBoard, obstacles: RoutingClearanceIndex, tracks: dict[int, TrackSegment],
             nets: tuple[str, ...], run: _Run, limit: int, gap: int | None, clearance_nm: int) -> list[_Window]:
    """The serpentine windows of one run (plan R10).

    Legs are a pitch of the lane width plus the leg gap apart, on a grid
    centred in the run 3 x width from its ends. Top i lies between legs i and
    i + 1; its room on each side is measured like a bump's over the outer
    member's legs, with the outline at the largest foot and least top chamfer
    any serpentine can use. For each phase (the side of the first grid top) a
    window is a maximal stretch of at least two tops with room on their sides.
    """
    first, second, _, low, high, distance = run
    width = max(tracks[first].width_nm, tracks[second].width_nm if second >= 0 else 0)
    gap = gap if gap is not None else max(3 * width, clearance_nm)
    pitch = distance + width + gap
    usable = high - low - 6 * width
    count = (usable - distance) // pitch
    if count < 2:
        return []
    offset = low + 3 * width + (usable - count * pitch - distance) // 2
    floor = max(4, _chamfer_offset(distance) + 2)
    chamfers = (width, 1 + _chamfer_offset(distance))
    tops = {(position, side): _run_slot(tracks, run, position, side, offset + position * pitch,
                                        offset + (position + 1) * pitch + distance, width)
            for position in range(count) for side in (1, -1)}
    rooms = {key: _room(board, obstacles, nets, slot, limit, floor, chamfers) for key, slot in tops.items()}
    windows = []
    for phase in (1, -1):
        sides = [phase if position % 2 == 0 else -phase for position in range(count)]
        position = 0
        while position < count:
            end = position
            while end < count and rooms[(end, sides[end])] > 0:
                end += 1
            if end - position >= 2:
                windows.append(_Window(tuple(tops[(index, sides[index])] for index in range(position, end)),
                                       tuple(rooms[(index, sides[index])] for index in range(position, end)),
                                       pitch, gap))
            position = end + 1
    return windows


def _cut(window: _Window, first: int, count: int) -> _Window:
    return replace(window, tops=window.tops[first:first + count], rooms=window.rooms[first:first + count])


def _span(window: _Window) -> _Slot:
    """A slot over the whole window, for its distance from the terminals."""
    return replace(window.tops[0], end=window.tops[-1].end)


def _parts(windows: list[_Window], heights: list[int]) -> list[list[int]]:
    """``heights`` split into each window's tops (bump heights follow)."""
    parts, used = [], 0
    for window in windows:
        parts.append(heights[used:used + len(window.tops)])
        used += len(window.tops)
    return parts


def _leg_loss(chamfer: int, spacing: int) -> int:
    """Length each member loses at the two corners of one serpentine leg.

    The member is inside one corner (chamfer leg c) and outside the other
    (c + d); each 45-degree piece replaces 2k of straight track with
    round(k sqrt 2).
    """
    loss = 0
    for leg in (chamfer, chamfer + _chamfer_offset(spacing)):
        loss += 2 * leg - round(hypot(leg, leg))
    return loss


def _window_chamfer(window: _Window, heights: tuple[int, ...] | list[int]) -> int:
    """The largest chamfer leg every leg of the window allows at ``heights``.

    R9's rule for a leg of length L: at most the width, L // 4 and
    (L - d) // 2; and at most (w + gap - 1) // 2, so the inner member's top
    keeps a straight piece. Half legs are as long as their top's height, the
    others as the two adjacent heights together. Below 1, no serpentine fits.
    """
    width, offset = window.tops[0].width, _chamfer_offset(window.tops[0].spacing)
    legs = (heights[0], *(a + b for a, b in zip(heights, heights[1:])), heights[-1])
    return min(width, (width + window.gap - 1) // 2,
               *(min(leg // 4, (leg - offset) // 2) for leg in legs))


def _window_capacity(window: _Window, heights: tuple[int, ...] | list[int]) -> int:
    """Length a serpentine of ``heights`` adds to each member (0 if it has no corners)."""
    chamfer = _window_chamfer(window, heights)
    if chamfer < 1:
        return 0
    return 2 * sum(heights) - (len(heights) + 1) * _leg_loss(chamfer, window.tops[0].spacing)


def _window_bounds(window: _Window) -> Bounds:
    """Bounds of a window's tops at full room, on both sides of its run."""
    areas = [_swept(top, room).bounds for top, room in zip(window.tops, window.rooms)]
    return Bounds(min(area.min_x for area in areas), min(area.min_y for area in areas),
                  max(area.max_x for area in areas), max(area.max_y for area in areas))


def _near(first: Bounds, second: Bounds, margin: int) -> bool:
    return Bounds(first.min_x - margin, first.min_y - margin,
                  first.max_x + margin, first.max_y + margin).intersects(second)


def serpentine_path(first_leg: int, pitch: int, spacing: int, sides: tuple[int, ...] | list[int],
                    heights: tuple[int, ...] | list[int], member: int, chamfer: int) -> list[tuple[int, int]]:
    """Centre-line corners ``(axial, offset)`` of one member's serpentine, in axial order.

    The pair's legs are ``pitch`` apart from ``first_leg``, the axial position
    of the lower member leg; top i lies toward ``sides[i]`` (+1 or -1 along
    the normal), ``heights[i]`` off each member's own lane. ``member`` is +1
    for the member whose lane lies toward +1 (or a single net, with
    ``spacing`` 0), else -1. On a leg moving toward its own side a member is
    the lower leg and inside the turn at the leg's first corner, otherwise
    the upper leg and inside at its second corner; inside corners take
    ``chamfer`` and outside ones ``chamfer`` + d (``bump_chamfers``).
    """
    offset = _chamfer_offset(spacing)
    rises = [0, *(side * height for side, height in zip(sides, heights)), 0]
    corners: list[tuple[int, int]] = []
    for leg in range(len(heights) + 1):
        before, after = rises[leg], rises[leg + 1]
        direction = 1 if after > before else -1
        along = first_leg + leg * pitch + (0 if member == direction else spacing)
        entry, leave = (chamfer, chamfer + offset) if member == direction else (chamfer + offset, chamfer)
        corners += [(along - entry, before), (along, before + direction * entry),
                    (along, after - direction * leave), (along + leave, after)]
    return corners


def _add_serpentine(features: dict[int, list[list[tuple[int, int]]]], tracks: dict[int, TrackSegment],
                    window: _Window, heights: list[int], chamfer: int) -> None:
    """Add a window's serpentine at ``heights`` to each member of its run."""
    first, second = window.tops[0].run
    sides = [top.side for top in window.tops]
    horizontal = window.tops[0].horizontal
    for index, other in ((first, second), (second, first)):
        if index < 0:
            continue
        member = 1 if other < 0 or (_normal(tracks[index].start, horizontal)
                                     > _normal(tracks[other].start, horizontal)) else -1
        features.setdefault(index, []).append(serpentine_path(
            window.tops[0].start, window.pitch, window.tops[0].spacing, sides, heights, member, chamfer))


def _swept(slot: _Slot, height: int, chamfers: tuple[int, int] | None = None) -> RoundedConvexShape:
    """Area covered by a bump's copper (every member) of ``height``.

    It is the convex outline of the bulge-side member's chamfered bump: its
    run chamfers are bridged by the straight edge from the foot to the leg's
    top end. The other member of a pair lies inside it, except where its run
    chamfers dip between the two lanes. ``chamfers`` (base, top) replace the
    bump's own, as for a serpentine top.
    """
    corners = bump_path(slot.start, slot.end, height,
                        *(chamfers or bump_chamfers(height, slot.width, slot.spacing)))
    if len(corners) == 8:
        corners = corners[:1] + corners[2:6] + corners[7:]

    def point(axial: int, rise: int) -> Point:
        normal = slot.base + slot.side * rise
        return Point(axial, normal) if slot.horizontal else Point(normal, axial)

    return RoundedConvexShape(tuple(point(*corner) for corner in corners), slot.width // 2)


def _room(board: PhysicalBoard, obstacles: RoutingClearanceIndex, nets: tuple[str, ...],
          slot: _Slot, limit: int, floor: int = 1, chamfers: tuple[int, int] | None = None) -> int:
    """Tallest clear bump height in ``floor..limit``, else 0 (the swept area only grows)."""
    def clear(height: int) -> bool:
        shape = _swept(slot, height, chamfers)
        # Each member is checked as its own net, so the other member's lands
        # count as foreign copper too.
        return (shape_in_board(board, shape, board.rules.minimum_clearance_nm,
                               board.rules.minimum_hole_clearance_nm)
                and all(obstacles.can_area(net, shape, slot.layer) for net in nets))

    if floor > limit or not clear(floor):
        return 0
    if clear(limit):
        return limit
    low, high = floor, limit
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
    return _near(_swept(slot, room).bounds, _swept(other, other_room).bounds,
                 slot.width // 2 + other.width // 2 + clearance_nm)


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


def _bumped(track: TrackSegment, features: list[list[tuple[int, int]]]) -> list[TrackSegment]:
    """Replace one axis-aligned segment with chamfered bumps or serpentines.

    Each feature is its centre-line corners ``(axial, offset)`` in axial
    order, ``offset`` along the normal from the segment's own line, so the
    inner member of a pair bends at its own lane. Features are spliced in
    travel order.
    """
    horizontal = _horizontal(track)
    assert horizontal is not None
    normal = _normal(track.start, horizontal)
    sign = 1 if _axial(track.end, horizontal) > _axial(track.start, horizontal) else -1

    def point(along: int, offset: int) -> Point:
        return Point(along, normal + offset) if horizontal else Point(normal + offset, along)

    points = [track.start]
    for corners in sorted(features, key=lambda corners: sign * corners[0][0]):
        points.extend(point(*corner) for corner in (corners if sign > 0 else reversed(corners)))
    points.append(track.end)
    return [TrackSegment(track.net, start, end, track.width_nm, track.layer)
            for start, end in zip(points, points[1:]) if start != end]
