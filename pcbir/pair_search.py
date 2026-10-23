"""Bounded same-layer joint package escape and octilinear pair search.

Both lanes follow one oriented spine. Pad fanout is selected jointly and turns
use offset-line intersections, not independent D+/D- maze routes. This module
proposes copper only: the critical stage owns profile/native-DRC acceptance.
Layer transitions and non-octilinear terminal rows deliberately fail closed.
"""
from __future__ import annotations

from dataclasses import dataclass, field, replace
from fractions import Fraction
from heapq import heappop, heappush
from itertools import product
from math import ceil, hypot, sqrt
from typing import Iterable, Iterator, NamedTuple

from .breakout import BreakoutRegions
from .geometry import RoundedConvexShape, shapes_clear
from .physical import BoardSide, CopperLayer, NetRoutingRule, PadKind, PhysicalBoard, Point, TrackSegment, Via, nm_from_mm
from .routing import GlobalNetRoute
from .routing_clearance import RoutingClearanceIndex
from .routing_clearance import RoutingBlocker
from .routing_layers import routing_layers
from .surface_path import _track_inside_board


_HEADS = ((1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1))
# Port lengths tried from a pair's land midpoint, shortest first (mm).
_PORT_LENGTHS = (.5, .75, 1, 1.5, 2, 3)


@dataclass(frozen=True, slots=True)
class _Port:
    center: Point
    heading: int  # outward from the package
    sign: int  # first member's side relative to the outward heading
    first: tuple[TrackSegment, ...]
    second: tuple[TrackSegment, ...]
    preference: int


@dataclass(frozen=True, slots=True)
class PairPortExample:
    center: Point
    heading: int
    sign: int
    can_depart: bool


@dataclass(frozen=True, slots=True)
class PairPortInventory:
    """Bounded proposal telemetry, not an accepted exit or search frontier."""
    component: str
    layer: CopperLayer
    pitch_nm: int
    port_count: int
    departable_count: int
    examples: tuple[PairPortExample, ...]


@dataclass(frozen=True, slots=True)
class PairTransitionExample:
    stage: str
    positions: tuple[Point, ...] = ()
    blockers: tuple[RoutingBlocker, ...] = ()
    local_expanded_states: int = 0
    local_discovered_nodes: int = 0


@dataclass(frozen=True, slots=True)
class PairTransitionInventory:
    component: str
    surface: CopperLayer
    middle_layer: CopperLayer
    pitch_nm: int
    transitions: int
    direct_contacts: int
    stage_counts: tuple[tuple[str, int], ...] = ()
    examples: tuple[PairTransitionExample, ...] = ()


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


class _PairAccessIndex(RoutingClearanceIndex):
    """Local pair paths emit exactly the differential profile's width pieces."""
    def route_pieces(self, net, start, end, width_nm, layer):
        return self.breakout.split_tracks((TrackSegment(net, start, end, width_nm, layer),), width_nm)

    def can_route(self, net, start, end, width_nm, layer):
        dx, dy = end.x_nm-start.x_nm, end.y_nm-start.y_nm
        if dx and dy and abs(dx) != abs(dy):
            return False
        return all(self.can_track(net, t.start, t.end, t.width_nm, layer)
                   for t in self.route_pieces(net, start, end, width_nm, layer))


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
    resumed_states: int = 0
    resumed_portals: int = 0
    port_inventory: list[PairPortInventory] = field(default_factory=list, repr=False, compare=False)
    transition_inventory: list[PairTransitionInventory] = field(default_factory=list, repr=False, compare=False)
    # Private exact frontiers; never report, fingerprint or accepted copper.
    _continuations: list = field(default_factory=list, repr=False, compare=False)


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


def _tracks(net: str, points: tuple[Point, ...], width: int, layer: CopperLayer,
            breakout: BreakoutRegions | None = None) -> tuple[TrackSegment, ...]:
    tracks = tuple(TrackSegment(net, a, b, width, layer)
                   for a, b in zip(points, points[1:]) if a != b)
    # Lanes of a net with breakout properties are cut at the region boundary
    # and carry the breakout width inside it, exactly as they will be emitted.
    return breakout.split_tracks(tracks, width) if breakout else tracks


def _legal(board: PhysicalBoard, index: RoutingClearanceIndex,
           first: tuple[TrackSegment, ...], second: tuple[TrackSegment, ...], clearance: int) -> bool:
    for track in (*first, *second):
        if (_heading(track.start, track.end) is None
                or not _track_inside_board(board, track.start, track.end, track.width_nm)
                or not index.can_track(track.net, track.start, track.end, track.width_nm, track.layer)):
            return False
    breakout = index.breakout
    if first and second and breakout.pair(first[0].net, second[0].net):
        # Members of a breakout pair are spaced by the pair gap, relaxed to
        # the breakout gap where each member's copper is inside its region.
        lands = {track: breakout.region(track.net, (track.start, track.end)) for track in (*first, *second)}
        return all(shapes_clear(RoundedConvexShape((a.start, a.end), a.width_nm // 2),
                                RoundedConvexShape((b.start, b.end), b.width_nm // 2),
                                breakout.spacing_nm(a.net, lands[a], b.net, lands[b]))
                   for a in first for b in second if a.layer is b.layer)
    return all(shapes_clear(RoundedConvexShape((a.start, a.end), a.width_nm // 2),
                            RoundedConvexShape((b.start, b.end), b.width_nm // 2), clearance)
               for a in first for b in second if a.layer is b.layer)


def _collinear_fork(first, second, center, heading, sign, offset, expansion=1):
    """One analytic wide-and-tapered collar for end-on cardinal lands.

    The direct narrow collar can pass too close to the other land. Depart
    transversely, turn 45 degrees, clear the farther land, then taper to the
    requested port pitch. Exact source width/gap and obstacle checks are still
    required. This replaces a failed slot; it adds no port or maze allowance.
    """
    if heading % 2:
        return None
    dx, dy = _HEADS[heading]
    nx, ny = -dy, dx
    if (first.x_nm-second.x_nm)*nx+(first.y_nm-second.y_nm)*ny:
        return None
    middle = Point((first.x_nm+second.x_nm)//2, (first.y_nm+second.y_nm)//2)
    spread = ceil(expansion * max(2*offset, ceil(hypot(first.x_nm-second.x_nm, first.y_nm-second.y_nm)/2)))
    far = max((point.x_nm-middle.x_nm)*dx+(point.y_nm-middle.y_nm)*dy
              for point in (first, second))
    wide_distance = far+spread+2*offset
    narrow_distance = wide_distance+spread-offset
    distance = (center.x_nm-middle.x_nm)*dx+(center.y_nm-middle.y_nm)*dy
    if distance <= narrow_distance:
        return None
    paths = []
    for point, lane in ((first, sign), (second, -sign)):
        wide = Point(middle.x_nm+dx*wide_distance+nx*lane*spread,
                     middle.y_nm+dy*wide_distance+ny*lane*spread)
        narrow = Point(middle.x_nm+dx*narrow_distance+nx*lane*offset,
                       middle.y_nm+dy*narrow_distance+ny*lane*offset)
        lead = spread // 2
        paths.append((point, Point(point.x_nm+nx*lane*lead, point.y_nm+ny*lane*lead),
                      Point(point.x_nm+dx*(spread-lead)+nx*lane*spread,
                            point.y_nm+dy*(spread-lead)+ny*lane*spread),
                      wide, narrow, Point(center.x_nm+nx*lane*offset, center.y_nm+ny*lane*offset)))
    return tuple(paths)


def _converging_port(index, first_name, second_name, first, second,
                     component, middle, heading, sign, distance, width, offset, layer):
    """Converge end-on round lands before turning both lanes outward.

    This is an analytic proposal for the existing heading/order/length slot,
    not extra maze work or acceptance authority. A round-land breakout and
    its actual local width/gap determine the narrow stems. Both lanes turn
    to one side of the land row, then taper to the requested port pitch only
    beyond the farther land. All emitted copper still needs the joint exact
    checks, including companion lands and opposite-face copper for transitions.
    """
    if heading % 2 or not index.breakout.pair(first_name, second_name):
        return None
    dx, dy = _HEADS[heading]
    nx, ny = -dy, dx
    if (first.x_nm-second.x_nm)*nx+(first.y_nm-second.y_nm)*ny:
        return None
    lands = tuple(index.breakout.region(net, (point,))
                  for net, point in ((first_name, first), (second_name, second)))
    for identity, point in zip(lands, (first, second)):
        if identity is None or not identity.startswith(f"pad:{component}."):
            return None
        shape = index.breakout.land(identity).shape
        if shape.spine != (point,) or shape.radius_nm <= 0:
            return None  # Not an actual round terminal; never reinterpret a via port.
    widths = tuple(index.breakout.track_width_nm(net, land, width)
                   for net, land in zip((first_name, second_name), lands))
    if widths[0] != widths[1]:
        return None
    gap = index.breakout.spacing_nm(first_name, lands[0], second_name, lands[1])
    neck = (widths[0]+gap+1)//2
    pitch = abs((first.x_nm-second.x_nm)*dx+(first.y_nm-second.y_nm)*dy)
    converge = pitch//2-neck
    diagonal = converge//3
    transverse = pitch//2-widths[0]//4
    if (diagonal <= 0 or offset < neck
            or transverse < diagonal+2*neck):
        return None
    lead = converge-diagonal
    far = max((p.x_nm-middle.x_nm)*dx+(p.y_nm-middle.y_nm)*dy
              for p in (first, second))
    run = far+pitch//2
    taper = offset-neck
    if distance <= run+taper:
        return None  # Preserve the selected length; never extend a short slot.
    first_near = ((first.x_nm-middle.x_nm)*dx+(first.y_nm-middle.y_nm)*dy) < 0
    side = sign if first_near else -sign
    center = Point(middle.x_nm+dx*distance+nx*side*transverse,
                   middle.y_nm+dy*distance+ny*side*transverse)
    paths = []
    for point, lane in ((first, sign), (second, -sign)):
        u = (point.x_nm-middle.x_nm)*dx+(point.y_nm-middle.y_nm)*dy
        inward = 1 if u < 0 else -1
        stem = u+inward*converge
        v = side*transverse+lane*neck
        def at(a, b):
            return Point(middle.x_nm+dx*a+nx*b, middle.y_nm+dy*a+ny*b)
        points = (point, at(u+inward*lead, 0), at(stem, side*diagonal),
                  at(stem, v-side*neck), at(stem+neck, v), at(run, v),
                  at(run+taper, side*transverse+lane*offset),
                  at(distance, side*transverse+lane*offset))
        paths.append(_tracks((first_name, second_name)[len(paths)], points,
                             width, layer, index.breakout))
    return _Port(center, heading, sign, *paths, -1)


def _ports(board: PhysicalBoard, index: RoutingClearanceIndex, first_name: str,
           second_name: str, first: Point, second: Point, component: str,
           width: int, offset: int, clearance: int, layer: CopperLayer, *,
           lengths: tuple[float, ...] = _PORT_LENGTHS,
           allow_local_fallback: bool = True) -> tuple[_Port, ...]:
    separation = _heading(second, first)
    if separation is None:
        return ()
    middle = Point((first.x_nm + second.x_nm) // 2, (first.y_nm + second.y_nm) // 2)
    placement = next(item for item in board.placements if item.reference == component)
    outward = Point(middle.x_nm - placement.position.x_nm, middle.y_nm - placement.position.y_nm)
    preferred = {(separation - 2) % 8, (separation + 2) % 8}
    # Side-by-side internal lands can need an oblique or staggered outward
    # collar. Sample all eight orientations, retaining the historical normal
    # exits first. Every complete collar still passes exact copper checks.
    headings = tuple(sorted(range(8), key=lambda h: (
        h not in preferred,
        -(_HEADS[h][0] * outward.x_nm + _HEADS[h][1] * outward.y_nm), h)))
    ports = []
    for preference, heading in enumerate(headings):
        normal = _normal(heading, offset)
        dot = ((first.x_nm - second.x_nm) * normal.x_nm
               + (first.y_nm - second.y_nm) * normal.y_nm)
        # Collinear lands have no imposed transverse order at this heading.
        # Either lane assignment can be physically legal; keep the historical
        # negative choice first, then check the other exact complete collar.
        signs = (1,) if dot > 0 else (-1,) if dot < 0 else (-1, 1)
        for sign, length in product(signs, lengths):
            step = nm_from_mm(length)
            dx, dy = _HEADS[heading]
            if dot == 0 and dx*outward.x_nm+dy*outward.y_nm > 0:
                converging = _converging_port(index, first_name, second_name,
                    first, second, component, middle, heading, sign, step, width, offset, layer)
                if converging is not None and _legal(
                        board, index, converging.first, converging.second, clearance):
                    ports.append(converging)
                    continue  # One admitted collar per existing slot, as before.
            center = Point(middle.x_nm + dx * step, middle.y_nm + dy * step)
            targets = (_shift(center, normal, sign), _shift(center, normal, -sign))
            admitted = False
            for a, b in product(_paths(first, targets[0]), _paths(second, targets[1])):
                # Fanout must reach the pair port in its stated orientation.
                if (_heading(a[-2], a[-1]) != heading
                        or _heading(b[-2], b[-1]) != heading):
                    continue
                first_tracks, second_tracks = (_tracks(first_name, a, width, layer, index.breakout),
                                               _tracks(second_name, b, width, layer, index.breakout))
                if _legal(board, index, first_tracks, second_tracks, clearance):
                    ports.append(_Port(center, heading, sign, first_tracks, second_tracks, preference))
                    admitted = True
                    break
            if not admitted and dot == 0:
                # Three analytic widths, at most one accepted collar in this
                # existing slot. The initial transverse lead clears square
                # companion lands before the diagonal outward departure.
                for expansion in (1, 1.5, 2):
                    fork = _collinear_fork(first, second, center, heading, sign, offset, expansion)
                    if fork is not None:
                        a, b = (_tracks(first_name, fork[0], width, layer, index.breakout),
                                _tracks(second_name, fork[1], width, layer, index.breakout))
                        if _legal(board, index, a, b, clearance):
                            ports.append(_Port(center, heading, sign, a, b, preference))
                            break
    ports.extend(_offset_round_ports(board, index, first_name, second_name, first, second,
                                    component, width, offset, clearance, layer, lengths, headings))
    if not ports and allow_local_fallback and index.breakout.pair(first_name, second_name):
        # Finite local alternative for recessed paired lands. A short final
        # segment fixes the joint search orientation; both lead-ins are exact
        # source-to-port copper, checked together before a port is offered.
        from .pin_access import local_access_path
        for preference, heading in enumerate(headings):
            if heading % 2:  # four cardinal portal orientations
                continue
            dx, dy = _HEADS[heading]
            normal = _normal(heading, offset)
            dot = ((first.x_nm-second.x_nm)*normal.x_nm
                   + (first.y_nm-second.y_nm)*normal.y_nm)
            signs = (1,) if dot > 0 else (-1,) if dot < 0 else (1, -1)
            for sign, length in product(signs, (1, 1.5, 2, 3, 4)):
                if sum(p.heading == heading for p in ports) >= 2:
                    break
                distance = nm_from_mm(length)
                center = Point(middle.x_nm+dx*distance, middle.y_nm+dy*distance)
                targets = (_shift(center, normal, sign), _shift(center, normal, -sign))
                approaches = tuple(Point(p.x_nm-dx*125000, p.y_nm-dy*125000) for p in targets)
                tails = (_tracks(first_name, (approaches[0], targets[0]), width, layer, index.breakout),
                         _tracks(second_name, (approaches[1], targets[1]), width, layer, index.breakout))
                if not _legal(board, index, *tails, clearance):
                    continue
                for order in ((0, 1), (1, 0)):
                    reserved = _PairAccessIndex(replace(board, tracks=(*board.tracks, *tails[0], *tails[1])))
                    paths = [None, None]
                    for member in order:
                        path = local_access_path(board, reserved,
                            (first_name, second_name)[member], (first, second)[member],
                            approaches[member], layer, width, step_nm=25000,
                            detour_nm=500000, maximum_states=2400)
                        if path is None:
                            break
                        paths[member] = (*path, *tails[member])
                        for track in path:
                            reserved.add_track(track, locked=True)
                    if all(p is not None for p in paths) and _legal(board, index, *paths, clearance):
                        ports.append(_Port(center, heading, sign, *paths, preference))
                        break
    return tuple(ports)


def _offset_round_ports(board, index, first_name, second_name, first, second,
                        component, width, offset, clearance, layer, lengths, headings):
    """At most two offset collars per cardinal heading for round plated lands.

    Foreign land edges suggest a lateral corridor; they never authorize copper.
    Analytic, source-width lead-ins and their final oriented tails are checked
    together against the unchanged complete board. No maze allowance is added.
    """
    if not index.breakout.pair(first_name, second_name):
        return ()
    from .drc import placed_pad_shape
    from .placement import transformed_local_point
    pose = next(p for p in board.placements if p.reference == component)
    footprint = board.footprints[pose.footprint]
    own = []
    foreign = []
    for pad in footprint.pads:
        position = transformed_local_point(pose, pad.position)
        if position in (first, second):
            if pad.kind is not PadKind.THROUGH_HOLE or pad.drill is None:
                return ()
            shape = placed_pad_shape(position, pad, pose)
            if shape.spine != (position,):
                return ()
            own.append(position)
        elif pad.kind is not PadKind.SMD or (pose.side is BoardSide.FRONT) == (layer is CopperLayer.FRONT):
            foreign.append(placed_pad_shape(position, pad, pose).bounds)
    if len(own) != 2:
        return ()
    middle = Point((first.x_nm+second.x_nm)//2, (first.y_nm+second.y_nm)//2)
    reach = max(nm_from_mm(value) for value in lengths)
    margin = offset+(width+1)//2+clearance+4
    results = []
    for heading in headings:
        if heading % 2:
            continue
        dx, dy = _HEADS[heading]
        nx, ny = -dy, dx
        dot = (first.x_nm-second.x_nm)*nx+(first.y_nm-second.y_nm)*ny
        if not dot:  # Existing converging/fork families own end-on contacts.
            continue
        sign = 1 if dot > 0 else -1
        shifts = set()
        for bounds in foreign:
            corners = ((x-middle.x_nm, y-middle.y_nm)
                       for x in (bounds.min_x,bounds.max_x) for y in (bounds.min_y,bounds.max_y))
            projected = tuple((x*dx+y*dy,x*nx+y*ny) for x,y in corners)
            if max(v for v,u in projected) < 0 or min(v for v,u in projected) > reach:
                continue
            for shift in (min(u for v,u in projected)-margin, max(u for v,u in projected)+margin):
                if 0 < abs(shift) <= reach:
                    shifts.add(shift)
        sides = {side:sorted((value for value in shifts if (value > 0) == (side > 0)),key=abs)[:2]
                 for side in (-1,1)}
        ordered = [values[rank] for rank in range(2) for values in sides.values() if rank < len(values)]
        admitted_sides = set()
        for shift in ordered:
            side = 1 if shift > 0 else -1
            if side in admitted_sides:
                continue
            for length in reversed(lengths):
                travel = nm_from_mm(length)
                center = Point(middle.x_nm+dx*travel+nx*shift,middle.y_nm+dy*travel+ny*shift)
                normal = _normal(heading,offset)
                targets = (_shift(center,normal,sign),_shift(center,normal,-sign))
                approaches = tuple(Point(p.x_nm-dx*125000,p.y_nm-dy*125000) for p in targets)
                for a,b in product(_paths(first,approaches[0]),_paths(second,approaches[1])):
                    paths = (_tracks(first_name,(*a,targets[0]),width,layer,index.breakout),
                             _tracks(second_name,(*b,targets[1]),width,layer,index.breakout))
                    if _legal(board,index,*paths,clearance):
                        results.append(_Port(center,heading,sign,*paths,-1))
                        admitted_sides.add(side)
                        break
                else:
                    continue
                break
            if len(admitted_sides) >= 2:
                break
    return tuple(results)


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


def _pair_profile(board: PhysicalBoard, first_rule: NetRoutingRule, second_rule: NetRoutingRule,
                  first_guide: GlobalNetRoute, second_guide: GlobalNetRoute,
                  ) -> tuple[int, int, int, CopperLayer, dict] | None:
    """Width, lane offset, clearance, surface layer and partner accesses; None when unsupported."""
    width = first_rule.width_nm or board.rules.default_track_width_nm
    if (second_rule.width_nm or board.rules.default_track_width_nm) != width:
        return None
    gap = first_rule.pair_gap_nm
    if gap is None or second_rule.pair_gap_nm != gap:
        return None
    if len(first_guide.accesses) != 2 or len(second_guide.accesses) != 2:
        return None
    from .pair_terminals import terminal_partners
    partners = terminal_partners(first_guide, second_guide)
    if partners is None:
        return None
    from .pair_terminals import terminal_copper_layers
    layers = set(routing_layers(board, first_rule.net, first_rule)).intersection(
        routing_layers(board, second_rule.net, second_rule))
    for access in (*first_guide.accesses, *second_guide.accesses):
        layers.intersection_update(terminal_copper_layers(board, access.pad))
    if not layers:
        return None
    # Keep a single bounded same-layer family. Other legal contact layers
    # can be considered by the transition search without inventing vias.
    layer = min(layers, key=board.stackup.copper_layers.index)
    offset = (width + gap + 1) // 2
    clearance = max(board.rules.minimum_clearance_nm, first_rule.clearance_nm or 0,
                    second_rule.clearance_nm or 0)
    return width, offset, clearance, layer, partners


def _departure_preferences(board, groups, first, second, width, offset,
                           clearance, layer, pitch):
    """Rank clear onward collars first without pruning short goal approaches.

    A full-pitch departure is mandatory at the start, but not at the goal:
    its last approach may be shorter. Use this bounded geometric lookahead
    only as a preference and retain every existing port combination. The
    caller still checks the complete escaped board before spending a slot.
    """
    preferences = {}
    for group in groups:
        for port in group:
            escaped = replace(board, tracks=(*board.tracks, *port.first, *port.second))
            preferences[id(port)] = int(not _can_depart(escaped,
                RoutingClearanceIndex(escaped), first, second, port,
                width, offset, clearance, layer, pitch))
    return preferences


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
    profile = _pair_profile(board, first_rule, second_rule, first_guide, second_guide)
    if profile is None:
        return
    width, offset, clearance, layer, partners = profile
    index = RoutingClearanceIndex(board)
    accesses = sorted(first_guide.accesses, key=lambda access: access.pad.component)
    groups = [_ports(board, index, first_rule.net, second_rule.net, access.pad_position,
                     partners[access.pad].pad_position, access.pad.component,
                     width, offset, clearance, layer) for access in accesses]
    onward = _departure_preferences(board, groups, first_rule.net, second_rule.net,
                                    width, offset, clearance, layer, pitch_nm)
    for access, ports in zip(accesses, groups):
        if len(stats.port_inventory) >= 24:
            break
        examples, per_heading = [], {}
        for port in ports:
            sample_key = (port.heading, port.preference)
            if per_heading.get(sample_key, 0) >= 2 or len(examples) >= 16:
                continue
            per_heading[sample_key] = per_heading.get(sample_key, 0) + 1
            examples.append(PairPortExample(port.center, port.heading, port.sign, onward[id(port)] == 0))
        stats.port_inventory.append(PairPortInventory(access.pad.component, layer, pitch_nm, len(ports),
            sum(onward[id(port)] == 0 for port in ports), tuple(examples)))
    combinations = sorted([(a, b) for a, b in product(*groups) if a.sign == -b.sign],
                          key=lambda pair: (onward[id(pair[0])] + onward[id(pair[1])],
                                            pair[0].preference + pair[1].preference,
                                            hypot(pair[0].center.x_nm - pair[1].center.x_nm,
                                                  pair[0].center.y_nm - pair[1].center.y_nm),
                                            pair[0].center.x_nm, pair[0].center.y_nm,
                                            pair[1].center.x_nm, pair[1].center.y_nm))
    stats.port_pairs += len(combinations)
    searches_at_entry = stats.searches
    for search_index, (start, end) in enumerate(combinations[:128], 1):
        if stats.searches-searches_at_entry >= maximum_searches:
            break
        budget = _remaining_states(stats, maximum_states, maximum_total_states)
        if not budget:
            break
        escaped = (*start.first, *start.second, *end.first, *end.second)
        if not _legal(board, index, (*start.first, *end.first), (*start.second, *end.second), clearance):
            continue
        escaped_board = replace(board, tracks=(*board.tracks, *escaped))
        escaped_index = RoutingClearanceIndex(escaped_board)
        if not _can_depart(escaped_board, escaped_index, first_rule.net, second_rule.net,
                           start, width, offset, clearance, layer, pitch_nm):
            continue
        stats.searches += 1
        candidate = _search(escaped_board, escaped_index, first_rule.net,
                            second_rule.net, start, end, width, offset, clearance, layer,
                            pitch_nm, budget, stats)
        from .pair_search_resume import register_pair_search
        register_pair_search(stats, board, retain_frontier=maximum_total_states is not None)
        if candidate is not None:
            stats.candidates += 1
            a, b, expanded, spine = candidate
            yield PairSearchCandidate((*start.first, *a, *tuple(
                TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer) for t in reversed(end.first))),
                (*start.second, *b, *tuple(
                TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer) for t in reversed(end.second))),
                expanded, search_index, spine, start, end)


def nested_turn_chamfer(width: int, gap: int) -> int:
    """Least axial leg of a nested exit's 45-degree turn: one pair pitch (plan R7)."""
    return width + gap


def nested_turn_minimum(width: int, gap: int) -> int:
    """Least turn distance of a nested exit from its land midpoint (plan R7).

    The shortest port, the least chamfer and one lane offset, so the inner
    lane's run between port and turn survives its miter.
    """
    return nm_from_mm(_PORT_LENGTHS[0]) + nested_turn_chamfer(width, gap) + (width + gap + 1) // 2


def nested_exit_candidates(board: PhysicalBoard, first_rule: NetRoutingRule, second_rule: NetRoutingRule,
                           first_guide: GlobalNetRoute, second_guide: GlobalNetRoute, component: str,
                           direction: tuple[int, int], travel: tuple[int, int], turns: Iterable[int],
                           chamfer_step: int) -> Iterator[tuple[int, int, PairSearchCandidate]]:
    """Coupled L-shaped candidates for a pair that wraps a package corner (plan R7).

    The pair leaves its lands at ``component`` along the axis ``direction``
    through its shortest legal port and turns 90 degrees onto its run along
    ``travel`` at each distance in ``turns`` from its land midpoint (the
    run's centre line). The turn is one 45-degree diagonal, longest first:
    from just after the port (the shortest route) down in ``chamfer_step``
    steps to one pair pitch (``nested_turn_chamfer``), so the first legal one
    hugs the copper inside the turn. The pair enters the far lands through
    their shortest legal port; a run beyond their column jogs back at 45
    degrees just before it. Only candidates whose lanes clear the board's
    copper are yielded, as (turn, chamfer, candidate); input copper is never
    modified.
    """
    if chamfer_step <= 0:
        raise ValueError("nested exit chamfer step must be positive")
    profile = _pair_profile(board, first_rule, second_rule, first_guide, second_guide)
    if profile is None or direction not in _HEADS[::2] or travel not in _HEADS[::2]:
        return
    width, offset, clearance, layer, partners = profile
    outward, inward = _HEADS.index(direction), _HEADS.index(travel)
    if _turn(outward, inward) != 2 or component not in partners:
        return
    index = RoutingClearanceIndex(board)
    ports = {access.pad.component: _ports(
        board, index, first_rule.net, second_rule.net, access.pad_position,
        partners[access.pad].pad_position, access.pad.component,
        width, offset, clearance, layer) for access in first_guide.accesses}
    start = next((port for port in ports[component] if port.heading == outward), None)
    end = next((port for name, items in ports.items() if name != component for port in items
                if start is not None and port.heading == (inward + 4) % 8 and port.sign == -start.sign),
               None)
    if start is None or end is None or not _legal(
            board, index, (*start.first, *end.first), (*start.second, *end.second), clearance):
        return
    escaped_board = replace(board, tracks=(*board.tracks, *start.first, *start.second,
                                           *end.first, *end.second))
    index = RoutingClearanceIndex(escaped_board)
    first_land = next(access.pad_position for access in first_guide.accesses
                      if access.pad.component == component)
    second_land = partners[component].pad_position
    middle = Point((first_land.x_nm + second_land.x_nm) // 2, (first_land.y_nm + second_land.y_nm) // 2)

    def at(across: int, along: int) -> Point:
        return Point(middle.x_nm + direction[0] * across + travel[0] * along,
                     middle.y_nm + direction[1] * across + travel[1] * along)

    def project(point: Point, axis: tuple[int, int]) -> int:
        return (point.x_nm - middle.x_nm) * axis[0] + (point.y_nm - middle.y_nm) * axis[1]

    port, column, reach = project(start.center, direction), project(end.center, direction), project(end.center, travel)
    least = nested_turn_chamfer(width, first_rule.pair_gap_nm)
    for turn in turns:
        jog = turn - column
        # The diagonal may start one lane offset after the port, where the
        # inner lane's miter still leaves it a straight piece.
        longest = min(turn - port - offset, (reach - jog - least if jog else reach) - least)
        if jog < 0 or longest < least:
            continue
        for chamfer in (*range(longest, least, -chamfer_step), least):
            points = [start.center, at(turn - chamfer, 0), at(turn, chamfer)]
            if jog:
                points += [at(turn, reach - jog - least), at(column, reach - least)]
            points.append(end.center)
            lanes = _lane_paths(tuple(points), outward, inward, offset, start.sign)
            if lanes is None:
                continue
            a = _tracks(first_rule.net, lanes[0], width, layer, index.breakout)
            b = _tracks(second_rule.net, lanes[1], width, layer, index.breakout)
            if not _legal(escaped_board, index, a, b, clearance):
                continue
            yield turn, chamfer, PairSearchCandidate(
                (*start.first, *a, *(TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer)
                                     for t in reversed(end.first))),
                (*start.second, *b, *(TrackSegment(t.net, t.end, t.start, t.width_nm, t.layer)
                                      for t in reversed(end.second))),
                0, 0, tuple(points), start, end)


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


def _can_depart(board, index, first_name, second_name, port, width, offset,
                clearance, layer, pitch):
    """Reject an impossible mandatory first step before spending a maze slot.

    The spine search requires its first step to follow the exact port heading
    for one pitch. This checks that same emitted geometry against the complete
    escaped board. It adds no paths, lookahead states or acceptance exemption.
    """
    dx, dy = _HEADS[port.heading]
    ahead = Point(port.center.x_nm + dx * pitch, port.center.y_nm + dy * pitch)
    lanes = _lane_paths((port.center, ahead), port.heading, port.heading, offset, port.sign)
    return lanes is not None and _legal(board, index,
        _tracks(first_name, lanes[0], width, layer, index.breakout),
        _tracks(second_name, lanes[1], width, layer, index.breakout), clearance)


def _search(board: PhysicalBoard, index: RoutingClearanceIndex, first_name: str, second_name: str,
            start: _Port, end: _Port, width: int, offset: int, clearance: int,
            layer: CopperLayer, pitch: int, budget: int, stats: PairSearchStats) -> _SpineSearchResult | None:
    from .pair_search_resume import _Continuation, _advance
    continuation = _Continuation(board, first_name, second_name, start, end, layer, pitch, stats.searches)
    continuation.steps = _search_steps(board, index, first_name, second_name,
        start, end, width, offset, clearance, layer, pitch, stats, continuation)
    stats._continuations.append(continuation)
    return _advance(continuation, budget)


def _search_steps(board: PhysicalBoard, index: RoutingClearanceIndex, first_name: str, second_name: str,
                  start: _Port, end: _Port, width: int, offset: int, clearance: int,
                  layer: CopperLayer, pitch: int, stats: PairSearchStats, continuation):
    """Yield after complete state expansions, retaining the exact A* frontier."""
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
                board, index, _tracks(first_name, lanes[0], width, layer, index.breakout),
                _tracks(second_name, lanes[1], width, layer, index.breakout), clearance)
        return legal_cache[key]

    while queue:
        _, cost, _, state = heappop(queue)
        if cost != best.get(state):
            continue
        point, heading = state
        expanded += 1
        stats.expanded_states += 1
        continuation.expanded_states = expanded
        continuation.minimum_goal_distance_nm = min(continuation.minimum_goal_distance_nm,
            hypot(point.x_nm-end.center.x_nm, point.y_nm-end.center.y_nm))
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
                a = _tracks(first_name, lanes[0], width, layer, index.breakout)
                b = _tracks(second_name, lanes[1], width, layer, index.breakout)
                if _legal(board, index, a, b, clearance):
                    continuation.active = False
                    yield _SpineSearchResult(a, b, expanded, points)
                    return
        for new in ((heading - 1) % 8, heading, (heading + 1) % 8):
            if state == initial and new != heading:
                continue  # The first lane endpoints must meet the exact port.
            if not legal_step(point, heading, new, state == initial):
                continue
            continuation.legal_advances += 1
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
        continuation.active = bool(queue)
        yield None
    continuation.active = False
