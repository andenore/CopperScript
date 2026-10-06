"""Breakout regions around a net's terminal lands (D-PHY plan R1).

A routing rule with ``breakout_length`` relaxes its width, pair gap and
clearance next to the net's terminal lands, where a fine-pitch pin field
cannot meet the channel values. Physical DRC, the routing clearance index and
the critical pair search share this one definition:

* The region of a terminal land is every point, in plan view and on every
  copper layer, within ``breakout_length`` of the land's copper outline. A
  point on the land is at distance 0. The region is the land swept by the
  breakout length, so it is convex.
* Copper is inside a region when its whole centreline is: both ends of a
  track segment, the centre of a via, every vertex of a pad outline's spine.
  By convexity the centreline between two inside ends is inside too. A track
  segment that crosses the boundary is outside; routers cut their tracks at
  the boundary (``split``) so the part next to the land is inside.
* Inside a region of its net, copper uses ``breakout_width``,
  ``breakout_gap`` and ``breakout_clearance`` (where declared) instead of
  ``width``, ``pair_gap`` and ``clearance``. Outside, the normal values apply.
* Two objects of different nets keep the larger of their nets' values and the
  board minimum clearance, each net's value taken at its own copper. Between
  the two members of a pair that declares breakout properties, that value is
  the pair gap instead of the clearance. Nets without breakout properties keep
  the plain clearance rule, including between pair members.
* Ordinary (``GENERAL``) nets neck down the same way: the detailed router,
  its pad access paths and the global pin access check and emit each
  centreline as its ``split_tracks`` pieces (``ordinary_pieces``), and cleanup
  re-cuts changed copper (``neck_down``). A piece is never wider than its
  check, so narrow copper is always inside a region.
"""

from __future__ import annotations

from dataclasses import dataclass
from fractions import Fraction
from typing import Iterable

from .geometry import Bounds, RoundedConvexShape, shape_distance_squared
from .physical import (CopperLayer, NetRoutingRule, PadKind, PadReference, PhysicalBoard, Point,
                       RouteKind, TrackSegment)
from .placement import transformed_local_point


@dataclass(frozen=True, slots=True)
class BreakoutLand:
    """One terminal land of a breakout net, with the bounds of its region."""

    identity: str  # "pad:REF.NUMBER:INDEX", the object name used by physical DRC
    net: str
    shape: RoundedConvexShape
    breakout_length_nm: int
    bounds: Bounds

    def distance_squared(self, point: Point) -> Fraction | None:
        """Squared spine distance of ``point`` when it is inside the region."""
        if not (self.bounds.min_x <= point.x_nm <= self.bounds.max_x
                and self.bounds.min_y <= point.y_nm <= self.bounds.max_y):
            return None
        reach = self.shape.radius_nm + self.breakout_length_nm
        distance = shape_distance_squared(RoundedConvexShape((point,)), self.shape)
        return distance if distance <= reach * reach else None

    def contains(self, point: Point) -> bool:
        return self.distance_squared(point) is not None


class BreakoutRegions:
    """Terminal-land regions of every net whose rule declares ``breakout_length``."""

    def __init__(self, board: PhysicalBoard) -> None:
        from .drc import placed_pad_shape  # physical DRC imports this module

        self.minimum_clearance_nm = board.rules.minimum_clearance_nm
        self.rules: dict[str, NetRoutingRule] = {rule.net: rule for rule in board.net_routing_rules}
        self.declared = frozenset(net for net, rule in self.rules.items()
                                  if rule.breakout_length_nm is not None)
        # Nets the ordinary detailed router owns; the critical router cuts its own.
        self.ordinary = frozenset(net for net in self.declared
                                  if self.rules[net].kind is RouteKind.GENERAL)
        self.default_track_width_nm = board.rules.default_track_width_nm
        lands: dict[str, list[BreakoutLand]] = {}
        if self.declared:
            assigned = {pad: net.name for net in board.nets if net.name in self.declared
                        for pad in net.pads}
            for placement in sorted(board.placements, key=lambda item: item.reference):
                for index, pad in enumerate(board.footprints[placement.footprint].pads):
                    net = assigned.get(PadReference(placement.reference, pad.number))
                    if net is None or pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                        continue
                    length = self.rules[net].breakout_length_nm
                    assert length is not None
                    shape = placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement)
                    lands.setdefault(net, []).append(BreakoutLand(
                        f"pad:{placement.reference}.{pad.number}:{index}", net, shape, length,
                        shape.bounds.expanded(length)))
        self._lands = {net: tuple(items) for net, items in lands.items()}
        self._by_identity = {land.identity: land for items in self._lands.values() for land in items}

    def __bool__(self) -> bool:
        return bool(self.declared)

    def land(self, identity: str) -> BreakoutLand:
        return self._by_identity[identity]

    def region(self, net: str, spine: Iterable[Point]) -> str | None:
        """The terminal land whose region holds every point of ``spine``.

        The nearest such land (by its farthest spine point) wins, then the
        first in placement order. ``None`` outside every region of ``net``.
        """
        lands = self._lands.get(net)
        if not lands:
            return None
        points = tuple(spine)
        best: tuple[Fraction, str] | None = None
        for land in lands:
            farthest = Fraction(0)
            for point in points:
                distance = land.distance_squared(point)
                if distance is None:
                    break
                farthest = max(farthest, distance)
            else:
                if best is None or farthest < best[0]:
                    best = (farthest, land.identity)
        return None if best is None else best[1]

    def split(self, net: str, start: Point, end: Point) -> tuple[tuple[Point, Point, str | None], ...]:
        """Cut one track centreline at the region boundaries of ``net``.

        Returns ``(start, end, land)`` pieces in order, ``land`` being the
        region that holds the piece (``None`` outside). Only octilinear
        segments are cut: their integer steps lie exactly on the centreline,
        so the pieces cover the same copper. A cut is at the last step inside
        a region, so the outside piece starts on the region's edge. A segment
        that is inside one region, or has no region at either end, is one
        piece; so is a segment of any other direction.
        """
        whole = ((start, end, self.region(net, (start, end))),)
        lands = self._lands.get(net)
        dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
        if (not lands or start == end or whole[0][2] is not None
                or (dx and dy and abs(dx) != abs(dy))):
            return whole
        steps = max(abs(dx), abs(dy))
        ux, uy = (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)

        def at(step: int) -> Point:
            return Point(start.x_nm + ux * step, start.y_nm + uy * step)

        def extent(origin: int, toward: int) -> int:
            # The farthest step toward ``toward`` that stays in a region holding
            # ``origin``; the steps between are inside because regions are convex.
            reached = origin
            for land in lands:
                if not land.contains(at(origin)):
                    continue
                inside, outside = origin, toward
                if land.contains(at(outside)):
                    inside = outside
                else:
                    while abs(outside - inside) > 1:
                        middle = (inside + outside) // 2
                        if land.contains(at(middle)):
                            inside = middle
                        else:
                            outside = middle
                reached = max(reached, inside) if toward > origin else min(reached, inside)
            return reached

        # ``head``: last step inside a region holding the start; ``tail``: first
        # step inside a region holding the end. When they overlap, the head
        # piece ends inside the end's region and one cut suffices.
        head, tail = extent(0, steps), extent(steps, 0)
        cuts = [step for step in ((head, tail) if head < tail else (head,)) if 0 < step < steps]
        if not cuts:
            return whole
        marks = (0, *cuts, steps)
        return tuple((at(a), at(b), self.region(net, (at(a), at(b))))
                     for a, b in zip(marks, marks[1:]) if a != b)

    def split_tracks(self, tracks: Iterable[TrackSegment], width_nm: int | None = None) -> tuple[TrackSegment, ...]:
        """Cut tracks at region boundaries; pieces inside get the breakout width.

        Pieces outside get ``width_nm`` when given (restoring a necked piece
        that now lies outside), otherwise the track's own width. Tracks of nets
        without breakout properties are returned unchanged.
        """
        result: list[TrackSegment] = []
        for track in tracks:
            if track.net not in self.declared:
                result.append(track)
                continue
            outside = track.width_nm if width_nm is None else width_nm
            for start, end, land in self.split(track.net, track.start, track.end):
                result.append(TrackSegment(track.net, start, end,
                                           self.track_width_nm(track.net, land, outside), track.layer))
        return tuple(result)

    def track_width_nm(self, net: str, land: str | None, width_nm: int) -> int:
        """The width routers give copper of ``net``: the breakout width inside a region."""
        rule = self.rules.get(net)
        if land is not None and rule is not None and rule.breakout_width_nm is not None:
            return rule.breakout_width_nm
        return width_nm

    def width_nm(self, net: str) -> int:
        """The profile width of ``net`` outside its regions: rule width, else board default."""
        rule = self.rules.get(net)
        return rule.width_nm if rule is not None and rule.width_nm else self.default_track_width_nm

    def ordinary_pieces(self, net: str, start: Point, end: Point, width_nm: int,
                        layer: CopperLayer) -> tuple[TrackSegment, ...]:
        """The copper the ordinary router checks and emits for one centreline.

        An ordinary net with breakout properties is cut at its region
        boundaries: ``breakout_width`` inside, ``width_nm`` outside. Any other
        centreline is one track of ``width_nm``.
        """
        track = TrackSegment(net, start, end, width_nm, layer)
        return self.split_tracks((track,), width_nm) if net in self.ordinary else (track,)

    def neck_down(self, tracks: Iterable[TrackSegment]) -> tuple[TrackSegment, ...]:
        """Re-cut ordinary copper after a geometry change such as pruning.

        Tracks of ordinary breakout nets are cut at region boundaries; inside
        pieces get the breakout width, outside pieces the profile width. Other
        tracks are unchanged. Routers keep narrow copper inside regions, so
        this only narrows checked copper.
        """
        result: list[TrackSegment] = []
        for track in tracks:
            if track.net in self.ordinary:
                result.extend(self.split_tracks((track,), self.width_nm(track.net)))
            else:
                result.append(track)
        return tuple(result)

    def required_width_nm(self, track: TrackSegment, width_nm: int) -> int:
        """The least width of verified ordinary copper: the breakout width inside a region."""
        if track.net not in self.ordinary:
            return width_nm
        return self.track_width_nm(track.net, self.region(track.net, (track.start, track.end)), width_nm)

    def pair(self, first: str, second: str) -> bool:
        """Whether two nets are the members of a pair that declares breakout properties."""
        a, b = self.rules.get(first), self.rules.get(second)
        return (a is not None and b is not None
                and (a.differential_partner == second or b.differential_partner == first)
                and (first in self.declared or second in self.declared)
                and a.pair_gap_nm is not None and b.pair_gap_nm is not None)

    def spacing_nm(self, first: str, first_land: str | None,
                   second: str, second_land: str | None) -> int:
        """Required copper spacing between objects of two different nets.

        ``first_land``/``second_land`` name the region holding each object's
        copper, ``None`` outside. Each net contributes its value at its own
        copper; the result is never below the board minimum clearance.
        """
        a, b = self.rules.get(first), self.rules.get(second)
        if self.pair(first, second):
            assert a is not None and b is not None
            return max(self.minimum_clearance_nm, _gap(a, first_land), _gap(b, second_land))
        return max(self.minimum_clearance_nm, _clearance(a, first_land), _clearance(b, second_land))

    def relaxes(self, net: str, land: str | None, check: str) -> bool:
        """Whether ``net`` declares the breakout value of ``check`` and its copper is inside."""
        rule = self.rules.get(net)
        if land is None or rule is None:
            return False
        return {"clearance": rule.breakout_clearance_nm, "pair_gap": rule.breakout_gap_nm,
                "track_width": rule.breakout_width_nm}[check] is not None


def _clearance(rule: NetRoutingRule | None, land: str | None) -> int:
    if rule is None:
        return 0
    if land is not None and rule.breakout_clearance_nm is not None:
        return rule.breakout_clearance_nm
    return rule.clearance_nm or 0


def _gap(rule: NetRoutingRule, land: str | None) -> int:
    if land is not None and rule.breakout_gap_nm is not None:
        return rule.breakout_gap_nm
    return rule.pair_gap_nm or 0
