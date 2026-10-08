"""Group serpentines: adjacent units of a length_match group tuned together (D-PHY plan R14).

Units (pairs or single nets) of one ``length_match`` group whose routing
rules name the same ``tuning_group`` are bent together, like the lanes of a
DDR byte lane: every lane of the group follows the same bumps or serpentine,
keeps its spacing to the next lane through every bend, and the group as a
whole uses the free room beside it, where each unit alone, hemmed in by its
neighbours, has none.

Lanes. The group's collinear pieces are joined into straight lines first
(``critical_tuning._straight_lines``). A *run* is a stretch along one axis and
layer where every net of the group has exactly one line, the lines of each
unit are adjacent (a pair at its lane spacing), and the lanes lie at offsets
o_1 < ... < o_N across it; W = o_N - o_1 is the lane span.

Legs. A bump or serpentine top lifts every lane by the same height h off its
own line, so the group adds the same length to every lane. A leg crosses all
lines together: on a leg rising toward + (along the normal, travelling up the
axis) lane N turns first and lane k (o_N - o_k) later, on a falling leg lane 1
first. Every lane thus keeps its distance to the others through the leg and
both its corners; a top's bulge lane (the outermost on its side) has the widest
top and the inner lane the narrowest. For a one-sided bump the bulge lane's
interval along the run is the inner lane's plus 2W.

Corners (plan R9 for N lanes). Every corner is a 45-degree chamfer. At the
lower end of a leg (toward -) lane N is inside the turn and lane k takes
chamfer leg c + U_k, U_k the sum of d(o_{j+1} - o_j) over the gaps between
lane k and lane N, d(s) = floor((2 - sqrt 2) s); at the upper end lane 1 is
inside and lane k takes c + V_k. So parallel pieces of adjacent lanes stay at
least their spacing apart, and every lane has U_k + V_k = D per leg: all lanes
gain the same true length. Each lane's offsets may be lowered by a few
nanometres (``lane_corners``) so that the rounded per-piece lengths agree
exactly too, so pairs keep their skew. A leg needs at least 2c + D of length:
a group bump is at least D + 2 nm high, and each leg costs every lane the same
f(lower) + f(upper), f(k) = 2k - round(k sqrt 2). For a pair alone the corners
are R9's.

Room and placement. A top's room is the tallest height, up to the smallest
``tuning_amplitude_limit`` of the members, at which the group's swept area
clears foreign copper, lands, keep-outs, holes and the board edge by the
applicable clearance: the convex outline of the bulge lane's chamfered top
(largest foot, least top chamfer) and the inner lane's feet, which holds every
lane's copper between them. Bumps (inner lane 3 x width wide, at least
3 x width from each other and the run's ends) lie on the densest grid,
centred in each run or shifted along it; most room first, then farthest from
the terminals. With ``tuning_style = "serpentine"`` on every member, tops lie
at a pitch of W + width + leg gap and a run takes one serpentine where
consecutive tops have room on alternating sides, as in plan R10, else bumps.
One chamfer leg c serves the whole step and heights are levelled, so the
length added is exact.

The critical router owns the targets, the order and the atomic validation
(``critical._tune_group``).
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from math import hypot

from .geometry import Bounds, RoundedConvexShape
from .mechanical import shape_in_board
from .physical import CopperLayer, PhysicalBoard, Point, TrackSegment, TuningStyle, Via
from .routing_clearance import RoutingClearanceIndex
from .critical_tuning import (_CHAMFER_STEPS, MATCH_TUNING_BUMP_LIMIT, MATCH_TUNING_LEG_LIMIT, UnitTuning, _axial,
                              _bumped, _chamfer_offset, _double, _horizontal, _levelled, _near, _normal,
                              _straight_lines, _terminal_ends)


# Pair members count as adjacent lanes at their lane spacing within this
# tolerance, as in R3's runs.
_LANE_TOLERANCE_NM = 4

# Nanometres by which ``lane_corners`` may lower a lane's corner offsets so
# that every lane's rounded length gain is equal.
_CORNER_SLACK_NM = 8

# Bound on the shifted bump grids of one run (``_bump_tops``).
_GRID_SHIFTS = 8


@dataclass(frozen=True, slots=True)
class _Lane:
    lead: int        # the lane's straight line in ``GroupTuner.segments``
    net: str
    offset: int      # normal coordinate of the line


@dataclass(frozen=True, slots=True)
class _Run:
    """A stretch where every lane of the group runs straight, side by side."""

    layer: CopperLayer
    horizontal: bool
    lanes: tuple[_Lane, ...]    # by ascending offset
    low: int
    high: int
    width: int                  # widest lane

    @property
    def span(self) -> int:
        return self.lanes[-1].offset - self.lanes[0].offset

    @property
    def gaps(self) -> tuple[int, ...]:
        return tuple(b.offset - a.offset for a, b in zip(self.lanes, self.lanes[1:]))

    @property
    def offset_sum(self) -> int:
        """D: the corner offset of the lane farthest from the inside of a turn."""
        return sum(_chamfer_offset(gap) for gap in self.gaps)


@dataclass(frozen=True, slots=True)
class _Top:
    """One bump, or one serpentine top, of a run."""

    run: int
    side: int          # +1 or -1 along the normal
    start: int         # the bulge lane's interval along the axis
    end: int
    inner: int         # the inner lane's interval width


@dataclass(frozen=True, slots=True)
class _Feature:
    """A bump (one top) or a serpentine (consecutive tops on alternating sides)."""

    tops: tuple[_Top, ...]
    rooms: tuple[int, ...]


def _diagonal_cost(leg: int) -> int:
    """Length a 45-degree chamfer with leg ``leg`` saves: 2k - round(k sqrt 2)."""
    return 2 * leg - round(hypot(leg, leg))


def lane_corners(gaps: tuple[int, ...], chamfer: int,
                 slack: int = _CORNER_SLACK_NM) -> tuple[tuple[int, int], ...] | None:
    """Chamfer legs ``(lower, upper)`` of every lane at each leg, or None.

    ``gaps`` are the distances between adjacent lanes (lane 1 first). Lane N
    is inside the turn at the lower end of a leg and takes ``chamfer``; lane k
    takes ``chamfer`` + U'_k, with U'_k - U'_{k+1} between 0 and
    d(gap) = floor((2 - sqrt 2) x gap) so adjacent lanes stay at least their
    gap apart; likewise V' from lane 1 at the upper end. The natural offsets
    (every step d) give every lane the same true length per leg; offsets at
    most ``slack`` below them are chosen, least in total and then by lane, so
    that every lane's rounded loss f(lower) + f(upper) is equal. None when no
    such offsets exist.
    """
    steps = [_chamfer_offset(gap) for gap in gaps]
    count = len(steps) + 1
    lower = [sum(steps[lane:]) for lane in range(count)]
    upper = [sum(steps[:lane]) for lane in range(count)]

    def loss(lane: int, low: int, up: int) -> int:
        return _diagonal_cost(chamfer + lower[lane] - low) + _diagonal_cost(chamfer + upper[lane] - up)

    best: tuple[int, tuple[tuple[int, int], ...]] | None = None
    for first in range(min(slack, lower[0]) + 1):
        target = loss(0, first, 0)
        # Lowered amounts (lower, upper) of the lane so far -> least total and path.
        states = {(first, 0): (first, ((first, 0),))}
        for lane in range(1, count):
            step = steps[lane - 1]
            following: dict[tuple[int, int], tuple[int, tuple[tuple[int, int], ...]]] = {}
            for (low, up), (total, path) in sorted(states.items()):
                for next_low in range(max(0, low - step), low + 1):
                    for next_up in range(up, min(slack, up + step) + 1):
                        if loss(lane, next_low, next_up) != target:
                            continue
                        entry = (total + next_low + next_up, (*path, (next_low, next_up)))
                        if (next_low, next_up) not in following or entry < following[(next_low, next_up)]:
                            following[(next_low, next_up)] = entry
            states = following
        for (low, _), entry in states.items():
            if low == 0 and (best is None or entry < best):
                best = entry
    if best is None:
        return None
    return tuple((chamfer + lower[lane] - low, chamfer + upper[lane] - up)
                 for lane, (low, up) in enumerate(best[1]))


def group_path(legs: list[int], rises: list[int], offsets: tuple[int, ...], lane: int,
               lower: int, upper: int) -> list[tuple[int, int]]:
    """Centre-line corners ``(axial, rise)`` of one lane of a group feature, in axial order.

    ``legs`` are the axial positions of each leg's first lane: lane N (the
    last of ``offsets``) on a leg rising toward +, lane 1 on a falling one.
    ``rises`` are the levels before, between and after the legs (0 on the
    line at both ends); every lane rises the same heights off its own line.
    ``lower``/``upper`` are the lane's chamfer legs at the lower and upper end
    of every leg (``lane_corners``).
    """
    corners: list[tuple[int, int]] = []
    for leg, position in enumerate(legs):
        before, after = rises[leg], rises[leg + 1]
        if after > before:
            along = position + offsets[-1] - offsets[lane]
            corners += [(along - lower, before), (along, before + lower),
                        (along, after - upper), (along + upper, after)]
        else:
            along = position + offsets[lane] - offsets[0]
            corners += [(along - upper, before), (along, before - upper),
                        (along, after + lower), (along + lower, after)]
    return corners


class GroupTuner:
    """Bumps and serpentines that bend every lane of a tuning group together.

    ``tracks`` is the copper of every unit of the group and ``obstacles``
    indexes all other copper. ``units`` are the group's units (each one net
    or a pair) and ``spacings`` each unit's lane spacing (0 for a single net).
    Rooms are measured once, so several length targets cost one survey.
    """

    def __init__(self, board: PhysicalBoard, obstacles: RoutingClearanceIndex,
                 tracks: tuple[TrackSegment, ...], units: tuple[tuple[str, ...], ...],
                 spacings: tuple[int, ...], amplitude_limit_nm: int, clearance_nm: int,
                 style: TuningStyle = TuningStyle.BUMPS, leg_gap_nm: int | None = None,
                 vias: tuple[Via, ...] = (), bump_limit: int = MATCH_TUNING_BUMP_LIMIT,
                 leg_limit: int = MATCH_TUNING_LEG_LIMIT) -> None:
        self.board, self.obstacles = board, obstacles
        self.tracks = tracks
        self.nets = tuple(net for unit in units for net in unit)
        self.limit = amplitude_limit_nm
        self.clearance_nm = clearance_nm
        self.bump_limit, self.leg_limit = bump_limit, leg_limit
        lines = _straight_lines(tracks, vias)
        self.segments = {lead: line for lead, (_, line) in lines.items()}
        self.lead_of = {index: lead for lead, (indices, _) in lines.items() for index in indices}
        self.runs = _runs(self.segments, units, spacings)
        self._corners: dict[tuple[tuple[int, ...], int], tuple[tuple[int, int], ...] | None] = {}
        ends = _terminal_ends(tracks)

        def remoteness(top: _Top) -> int:
            run = self.runs[top.run]
            bulge = run.lanes[-1] if top.side > 0 else run.lanes[0]
            middle = (top.start + top.end) // 2
            point = Point(middle, bulge.offset) if run.horizontal else Point(bulge.offset, middle)
            return min(((point.x_nm - end.x_nm) ** 2 + (point.y_nm - end.y_nm) ** 2 for end in ends), default=0)

        def order(top: _Top) -> tuple:
            run = self.runs[top.run]
            return (-remoteness(top), run.layer.value, not run.horizontal, run.lanes[0].offset, top.start, top.side)

        self.remoteness = remoteness
        self.rooms: dict[_Top, int] = {}
        for index, run in enumerate(self.runs):
            for top in _bump_tops(index, run):
                self.rooms[top] = self._room(top)
        self.ordered = sorted((top for top, room in self.rooms.items() if room > 0),
                              key=lambda top: (-self.rooms[top], *order(top)))
        self.windows: list[_Feature] = []
        if TuningStyle(style) is TuningStyle.SERPENTINE:
            for index, run in enumerate(self.runs):
                gap = leg_gap_nm if leg_gap_nm is not None else max(3 * run.width, clearance_nm)
                tops = _serpentine_tops(index, run, gap)
                rooms = {top: self._room(top) for top in tops}
                self.windows.extend(_windows(tops, rooms))
            self.windows.sort(key=lambda window: (-self._capacity(window, window.rooms), *order(_span(window))))

    def plan(self, added_nm: int) -> UnitTuning:
        """Add ``added_nm`` (even) to every lane: serpentines first where declared, else bumps."""
        if added_nm <= 0:
            return UnitTuning(None, reason="no length to add")
        if not self.runs:
            return UnitTuning(None, reason=f"no straight run on which all {len(self.nets)} lanes "
                                           "run side by side")
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
        chosen: list[_Top] = []
        capacity = 0
        for top in self.ordered:
            if capacity >= added_nm or len(chosen) >= self.bump_limit:
                break
            if any(self._conflict(top, other) for other in chosen):
                continue
            chosen.append(top)
            capacity += self._capacity(_Feature((top,), (self.rooms[top],)), (self.rooms[top],))
        if capacity < added_nm:
            limit = f", the limit of {self.bump_limit} bumps" if len(chosen) >= self.bump_limit else ""
            return UnitTuning(None, reason=f"insufficient tuning room: {capacity} of {added_nm} nm "
                                           f"in {len(chosen)} group bump(s){limit}; {self._diagnosis()}")
        return self._build([_Feature((top,), (self.rooms[top],)) for top in chosen], added_nm)

    def _serpentines(self, added_nm: int) -> UnitTuning:
        chosen: list[_Feature] = []
        legs = capacity = 0
        for window in self.windows:
            if capacity >= added_nm or legs + 3 > self.leg_limit:
                break
            if any(window.tops[0].run == other.tops[0].run
                   or _near(self._bounds(window), self._bounds(other), self._margin(window, other))
                   for other in chosen):
                continue
            window = self._trimmed(window, added_nm - capacity, self.leg_limit - legs)
            chosen.append(window)
            legs += len(window.tops) + 1
            capacity += self._capacity(window, window.rooms)
        bumps: list[_Top] = []
        runs = {window.tops[0].run for window in chosen}
        for top in self.ordered:
            if not chosen or capacity >= added_nm or legs + 2 > self.leg_limit:
                break
            single = _Feature((top,), (self.rooms[top],))
            if (top.run in runs
                    or any(_near(self._bounds(single), self._bounds(window), self._margin(single, window))
                           for window in chosen)
                    or any(self._conflict(top, other) for other in bumps)):
                continue
            bumps.append(top)
            legs += 2
            capacity += self._capacity(single, single.rooms)
        if not chosen or capacity < added_nm:
            limit = f", the limit of {self.leg_limit} legs" if legs + 2 > self.leg_limit else ""
            return UnitTuning(None, reason=f"insufficient tuning room: {capacity} of {added_nm} nm "
                                           f"in {legs} group leg(s){limit}")
        return self._build([*chosen, *(_Feature((top,), (self.rooms[top],)) for top in bumps)], added_nm)

    def _trimmed(self, window: _Feature, need: int, legs: int) -> _Feature:
        """The fewest consecutive tops of ``window`` that add ``need``, within ``legs`` legs."""
        most = min(len(window.tops), legs - 1)
        for count in range(2, most + 1):
            first = max(range(len(window.tops) - count + 1), key=lambda first: (
                self._capacity(_cut(window, first, count), window.rooms[first:first + count]),
                self.remoteness(_span(_cut(window, first, count))), -first))
            part = _cut(window, first, count)
            if count == most or self._capacity(part, part.rooms) >= need:
                return part
        return window

    def _build(self, features: list[_Feature], added_nm: int) -> UnitTuning:
        """The features at levelled heights, with one chamfer leg for every leg, adding exactly ``added_nm``."""
        rooms = [room for feature in features for room in feature.rooms]
        chamfer, heights = min(self._chamfer(feature, feature.rooms) for feature in features), rooms
        for _ in range(_CHAMFER_STEPS):
            if chamfer < 1:
                break
            losses = [self._leg_loss(feature, chamfer) for feature in features]
            if None in losses:
                chamfer -= 1
                continue
            total = added_nm + sum((len(feature.tops) + 1) * loss for feature, loss in zip(features, losses))
            if total % 2:
                chamfer -= 1
                continue
            heights = _levelled(rooms, total, [_double] * len(rooms))
            fitting = min(self._chamfer(feature, part) for feature, part in zip(features, _parts(features, heights)))
            if fitting >= chamfer:
                break
            chamfer = fitting
        else:
            chamfer = 0
        if chamfer < 1:
            run = self.runs[features[0].tops[0].run]
            return UnitTuning(None, reason=f"{_mm(added_nm)} mm does not split into group tops of at least "
                                           f"{_mm(_floor(run))} mm, the least with 45-degree corners")
        lines: dict[int, list[list[tuple[int, int]]]] = {}
        amplitudes: list[int] = []
        bumps = legs = 0
        for feature, part in zip(features, _parts(features, heights)):
            run = self.runs[feature.tops[0].run]
            corners = self._lane_corners(run, chamfer)
            assert corners is not None
            positions = [top.start for top in feature.tops] + [feature.tops[-1].end - run.span]
            rises = [0, *(top.side * height for top, height in zip(feature.tops, part)), 0]
            offsets = tuple(lane.offset for lane in run.lanes)
            for index, lane in enumerate(run.lanes):
                lines.setdefault(lane.lead, []).append(group_path(positions, rises, offsets, index, *corners[index]))
            amplitudes.extend(part)
            if len(feature.tops) == 1:
                bumps += 1
            else:
                legs += len(feature.tops) + 1
        lanes = tuple(lane.net for lane in self.runs[features[0].tops[0].run].lanes)
        return UnitTuning(self._spliced(lines), bumps, legs, tuple(amplitudes), lanes=lanes)

    def _spliced(self, features: dict[int, list[list[tuple[int, int]]]]) -> tuple[TrackSegment, ...]:
        """The group's copper in the original track order, each tuned line replacing its pieces."""
        tuned: list[TrackSegment] = []
        for index, track in enumerate(self.tracks):
            lead = self.lead_of.get(index, index)
            if lead not in features:
                tuned.append(track)
            elif index == lead:
                tuned.extend(_bumped(self.segments[lead], features[lead]))
        return tuple(tuned)

    def _lane_corners(self, run: _Run, chamfer: int) -> tuple[tuple[int, int], ...] | None:
        key = (run.gaps, chamfer)
        if key not in self._corners:
            self._corners[key] = lane_corners(run.gaps, chamfer)
        return self._corners[key]

    def _leg_loss(self, feature: _Feature, chamfer: int) -> int | None:
        """Length every lane loses at the two corners of one leg of ``feature``."""
        corners = self._lane_corners(self.runs[feature.tops[0].run], chamfer)
        return None if corners is None else _diagonal_cost(corners[0][0]) + _diagonal_cost(corners[0][1])

    def _chamfer(self, feature: _Feature, heights: tuple[int, ...] | list[int]) -> int:
        """The largest chamfer leg every leg of ``feature`` allows at ``heights``.

        R9's rule on each leg of length L: at most the width, L // 4 and
        (L - D) // 2, and small enough to leave the inner lane a straight top.
        Half legs are as long as their top's height, the others as the two
        adjacent heights together. Below 1, the feature does not fit.
        """
        run = self.runs[feature.tops[0].run]
        legs = (heights[0], *(a + b for a, b in zip(heights, heights[1:])), heights[-1])
        return min(run.width, (feature.tops[0].inner - 1) // 2,
                   *(min(leg // 4, (leg - run.offset_sum) // 2) for leg in legs))

    def _capacity(self, feature: _Feature, heights: tuple[int, ...] | list[int]) -> int:
        """Length a feature at ``heights`` adds to every lane (0 if it has no corners)."""
        chamfer = self._chamfer(feature, heights)
        while chamfer >= 1:
            loss = self._leg_loss(feature, chamfer)
            if loss is not None:
                return 2 * sum(heights) - (len(heights) + 1) * loss
            chamfer -= 1
        return 0

    def _room(self, top: _Top) -> int:
        """Tallest clear height of ``top`` in floor..limit, else 0 (the swept area only grows)."""
        run = self.runs[top.run]
        floor = _floor(run)

        def clear(height: int) -> bool:
            shape = _swept(run, top, height)
            return (shape_in_board(self.board, shape, self.board.rules.minimum_clearance_nm,
                                   self.board.rules.minimum_hole_clearance_nm)
                    and all(self.obstacles.can_area(net, shape, run.layer) for net in self.nets))

        if floor > self.limit or not clear(floor):
            return 0
        if clear(self.limit):
            return self.limit
        low, high = floor, self.limit
        while high - low > 1:
            middle = (low + high) // 2
            low, high = (middle, high) if clear(middle) else (low, middle)
        return low

    def _bounds(self, feature: _Feature) -> Bounds:
        areas = [_swept(self.runs[top.run], top, room).bounds for top, room in zip(feature.tops, feature.rooms)]
        return Bounds(min(area.min_x for area in areas), min(area.min_y for area in areas),
                      max(area.max_x for area in areas), max(area.max_y for area in areas))

    def _margin(self, first: _Feature, second: _Feature) -> int:
        return (self.runs[first.tops[0].run].width // 2 + self.runs[second.tops[0].run].width // 2
                + self.clearance_nm)

    def _conflict(self, top: _Top, other: _Top) -> bool:
        """Two bumps cannot both be used: too close on one run, or their swept areas near."""
        run = self.runs[top.run]
        if top.run == other.run:
            pitch = 3 * run.width
            return top.start < other.end + pitch and other.start < top.end + pitch
        if run.layer is not self.runs[other.run].layer:
            return False
        first, second = _Feature((top,), (self.rooms[top],)), _Feature((other,), (self.rooms[other],))
        return _near(self._bounds(first), self._bounds(second), self._margin(first, second))

    def _diagnosis(self) -> str:
        """Why the group's bumps fall short: run length, amplitude or room per side."""
        def length(run: _Run) -> int:
            return run.high - run.low

        longest = max(self.runs, key=lambda run: (length(run), -self.runs.index(run)))
        inner, margin = 3 * longest.width, 3 * longest.width
        needed = inner + 2 * longest.span + 2 * margin
        if all(length(run) < 3 * run.width + 2 * run.span + 6 * run.width for run in self.runs):
            return (f"the longest straight run of all {len(longest.lanes)} lanes is {_mm(length(longest))} mm, "
                    f"a one-sided group bump needs {_mm(needed)} mm: the inner lane's {_mm(inner)} mm plus "
                    f"twice the {_mm(longest.span)} mm lane span, and {_mm(margin)} mm at each end")
        if self.limit < _floor(longest):
            return (f"tuning_amplitude_limit {_mm(self.limit)} mm is below the least group bump height "
                    f"{_mm(_floor(longest))} mm (45-degree corners across the {_mm(longest.span)} mm lane span)")
        sides: dict[str, int] = {}
        for top, room in self.rooms.items():
            run = self.runs[top.run]
            label = ("+" if top.side > 0 else "-") + ("y" if run.horizontal else "x")
            sides[label] = max(sides.get(label, 0), room)
        rooms = ", ".join(f"{label} {_mm(room)} mm" for label, room in sorted(sides.items()))
        return f"room beside the lanes: {rooms} (a group bump needs at least {_mm(_floor(longest))} mm)"


def _mm(value: int) -> str:
    return f"{value / 1e6:.3f}"


def _floor(run: _Run) -> int:
    """Least height of a top whose legs have room for 45-degree corners: c >= 1."""
    return max(4, run.offset_sum + 2)


def _runs(segments: dict[int, TrackSegment], units: tuple[tuple[str, ...], ...],
          spacings: tuple[int, ...]) -> list[_Run]:
    """Stretches where every net of the group has one straight line, side by side.

    Along each axis and layer, the lines are cut at all their ends; a stretch
    where each net has exactly one line covering it, and each unit's lanes are
    adjacent (a pair's at its lane spacing), is a run, joined with the next
    stretch when the lines are the same.
    """
    nets = sorted(net for unit in units for net in unit)
    axes: dict[tuple[str, bool], list[int]] = {}
    for lead, line in segments.items():
        horizontal = _horizontal(line)
        if horizontal is not None and line.net in nets and line.start != line.end:
            axes.setdefault((line.layer.value, horizontal), []).append(lead)
    runs: list[_Run] = []
    for (_, horizontal), leads in sorted(axes.items()):
        spans = {lead: sorted((_axial(segments[lead].start, horizontal), _axial(segments[lead].end, horizontal)))
                 for lead in leads}
        cuts = sorted({value for span in spans.values() for value in span})
        current: list | None = None
        for low, high in zip(cuts, cuts[1:]):
            covering = [lead for lead in leads if spans[lead][0] <= low and spans[lead][1] >= high]
            lanes = None
            if sorted(segments[lead].net for lead in covering) == nets:
                lanes = tuple(sorted((_Lane(lead, segments[lead].net, _normal(segments[lead].start, horizontal))
                                      for lead in covering), key=lambda lane: (lane.offset, lane.net)))
                if not _adjacent(lanes, units, spacings):
                    lanes = None
            if lanes is not None and current is not None and current[0] == lanes and current[2] == low:
                current[2] = high
                continue
            if current is not None:
                runs.append(_run(segments, horizontal, *current))
            current = [lanes, low, high] if lanes is not None else None
        if current is not None:
            runs.append(_run(segments, horizontal, *current))
    return runs


def _run(segments: dict[int, TrackSegment], horizontal: bool, lanes: tuple[_Lane, ...], low: int,
         high: int) -> _Run:
    return _Run(segments[lanes[0].lead].layer, horizontal, lanes, low, high,
                max(segments[lane.lead].width_nm for lane in lanes))


def _adjacent(lanes: tuple[_Lane, ...], units: tuple[tuple[str, ...], ...], spacings: tuple[int, ...]) -> bool:
    """Every unit's lanes are neighbours (a pair's at its spacing), all at distinct offsets."""
    if any(a.offset == b.offset for a, b in zip(lanes, lanes[1:])):
        return False
    position = {lane.net: index for index, lane in enumerate(lanes)}
    for unit, spacing in zip(units, spacings):
        indices = sorted(position[net] for net in unit)
        if indices[-1] - indices[0] != len(indices) - 1:
            return False
        if len(unit) == 2 and abs(lanes[indices[1]].offset - lanes[indices[0]].offset - spacing) > _LANE_TOLERANCE_NM:
            return False
    return True


def _bump_tops(index: int, run: _Run) -> list[_Top]:
    """One-sided bump slots of a run on both sides.

    Bumps lie on the densest grid (bump, then 3 x width, repeated), centred
    in the run, and on grids shifted from one end to the other at steps of at
    least 3 x width (at most ``_GRID_SHIFTS`` + 1 of them), since a group
    bump is long and the room beside a run often ends before the run does.
    """
    width, span = run.width, run.span
    pitch = 3 * width
    bump = pitch + 2 * span
    usable = run.high - run.low - 2 * pitch
    count = (usable + pitch) // (bump + pitch) if usable >= bump else 0
    if count == 0:
        return []
    slack = usable - count * bump - (count - 1) * pitch
    step = max(pitch, slack // _GRID_SHIFTS, 1)
    shifts = sorted({slack // 2, *range(0, slack + 1, step), slack})
    tops = []
    for shift in shifts:
        for position in range(count):
            start = run.low + pitch + shift + position * (bump + pitch)
            for side in (1, -1):
                tops.append(_Top(index, side, start, start + bump, pitch))
    return tops


def _serpentine_tops(index: int, run: _Run, gap: int) -> list[_Top]:
    """Serpentine tops of a run: legs a pitch of W + width + gap apart, on a grid centred in the run."""
    width, span = run.width, run.span
    pitch = span + width + gap
    usable = run.high - run.low - 6 * width
    count = (usable - span) // pitch
    if count < 2:
        return []
    first = run.low + 3 * width + (usable - count * pitch - span) // 2
    return [_Top(index, side, first + position * pitch, first + (position + 1) * pitch + span, width + gap)
            for position in range(count) for side in (1, -1)]


def _windows(tops: list[_Top], rooms: dict[_Top, int]) -> list[_Feature]:
    """For each phase, maximal stretches of two or more consecutive tops with room on alternating sides."""
    count = len(tops) // 2
    by_position = {(position, top.side): top for position, top in
                   ((index // 2, top) for index, top in enumerate(tops))}
    windows = []
    for phase in (1, -1):
        sides = [phase if position % 2 == 0 else -phase for position in range(count)]
        position = 0
        while position < count:
            end = position
            while end < count and rooms[by_position[(end, sides[end])]] > 0:
                end += 1
            if end - position >= 2:
                chosen = tuple(by_position[(index, sides[index])] for index in range(position, end))
                windows.append(_Feature(chosen, tuple(rooms[top] for top in chosen)))
            position = end + 1
    return windows


def _cut(window: _Feature, first: int, count: int) -> _Feature:
    return _Feature(window.tops[first:first + count], window.rooms[first:first + count])


def _span(window: _Feature) -> _Top:
    """A top over the whole feature, for its distance from the terminals."""
    return replace(window.tops[0], end=window.tops[-1].end)


def _parts(features: list[_Feature], heights: list[int]) -> list[list[int]]:
    parts, used = [], 0
    for feature in features:
        parts.append(heights[used:used + len(feature.tops)])
        used += len(feature.tops)
    return parts


def _swept(run: _Run, top: _Top, height: int) -> RoundedConvexShape:
    """Area covered by every lane's copper of ``top`` at ``height``.

    The convex outline of the bulge lane's chamfered top, at the largest foot
    chamfer (the width) and the least top chamfer any step can use, and of the
    inner lane's feet at their largest chamfer: every other lane's copper lies
    between them.
    """
    span, offset = run.span, run.offset_sum
    bulge = run.lanes[-1] if top.side > 0 else run.lanes[0]
    base, far = bulge.offset, bulge.offset - top.side * span
    foot, crown, far_foot = run.width, max(1, 1 + offset - _CORNER_SLACK_NM), run.width + offset
    rise = base + top.side * height
    corners = ((top.start + span - far_foot, far), (top.start - foot, base),
               (top.start, rise - top.side * crown), (top.start + crown, rise),
               (top.end - crown, rise), (top.end, rise - top.side * crown),
               (top.end + foot, base), (top.end - span + far_foot, far))

    def point(axial: int, normal: int) -> Point:
        return Point(axial, normal) if run.horizontal else Point(normal, axial)

    return RoundedConvexShape(tuple(dict.fromkeys(point(*corner) for corner in corners)), run.width // 2)
