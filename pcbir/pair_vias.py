"""Bounded paired terminal transitions plus a joint single-layer middle route.

Short surface collars widen to manufacturing-legal matched through-vias;
the other signal layer tapers back to the declared pair spacing. Both lanes
are searched together. This is not arbitrary 3-D maze routing or SI signoff.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, field, fields, replace
from itertools import product
from math import hypot, sqrt
from typing import Callable, Iterator

from .pair_search import (PairSearchCandidate, PairSearchStats, PairTransitionInventory, PairTransitionExample, _Port, _HEADS,
                          _legal, _ports, _search, _remaining_states, _can_depart,
                          _PairAccessIndex, _normal, _shift, _tracks)
from .pair_search import _complete_support_port
from .drc import placed_pad_shape
from .geometry import RoundedConvexShape, shapes_clear
from .placement import transformed_local_point
from .physical import (BoardSide, CopperLayer, NetRoutingRule, PhysicalBoard,
                       PadKind, PadReference, Point, Via, nm_from_mm)
from .routing import GlobalNetRoute
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_vias import physical_via_span
from .surface_path import via_inside_board
from .return_paths import shared_reference_plane


@dataclass(frozen=True, slots=True)
class _Transition:
    port: _Port
    vias: tuple[Via, ...]
    returns: tuple[Via, ...]
    # Caller's site preference (lower first); 0 unless a site rank is given.
    rank: int = 0
    outer: bool = False


@dataclass(slots=True)
class _TransitionTrace:
    """Finite diagnostic samples, with no influence on candidate admission."""
    counts: Counter = field(default_factory=Counter)
    examples: list = field(default_factory=list)

    def record(self, stage, *, positions=(), index=None, vias=(), tracks=(), local=None):
        self.counts[stage] += 1
        if (len(self.examples) >= 16 or
                sum(item.stage == stage for item in self.examples) >= 2):
            return
        blockers = {}
        if index is not None:
            for item in vias:
                for blocker in index.via_blocker_records(item):
                    blockers.setdefault((blocker.identity, blocker.occurrence), blocker)
            for item in tracks:
                for blocker in index.track_blocker_records(item):
                    blockers.setdefault((blocker.identity, blocker.occurrence), blocker)
        self.examples.append(PairTransitionExample(stage, tuple(positions),
            tuple(blockers.values())[:8], local.expanded_states if local else 0,
            local.discovered_nodes if local else 0))


def transition_spacing(board: PhysicalBoard, first: NetRoutingRule,
                       second: NetRoutingRule) -> int:
    """Via pads/drills need wider spacing than the coupled trace pitch.

    The members of a pair that declares breakout properties are spaced by the
    pair gap instead of the clearance (plan R1), their vias included.
    """
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0,
                    second.clearance_nm or 0)
    if ((first.breakout_length_nm is not None or second.breakout_length_nm is not None)
            and first.pair_gap_nm is not None and second.pair_gap_nm is not None):
        clearance = max(clearance, first.pair_gap_nm, second.pair_gap_nm)
    width = first.width_nm or board.rules.default_track_width_nm
    return max(width + (first.pair_gap_nm or 0),
               board.rules.default_via_size_nm + clearance + 4,
               board.rules.default_via_drill_nm + board.rules.minimum_hole_clearance_nm + 4)


def _reserve_vias(board: PhysicalBoard, index: RoutingClearanceIndex,
                  vias: tuple[Via, ...]) -> bool:
    for via in vias:
        if (not via_inside_board(board, via.position, via.size_nm)
                or not index.can_via(via.net, via.position, via.size_nm,
                                     via.from_layer, via.to_layer, via.drill_nm,
                                     check_hole_copper=True)):
            return False
        index.add_via(via, locked=True)
    return True


def _signal_drills_outside_smd(board: PhysicalBoard, pair: tuple[Via, Via]) -> bool:
    """Do not introduce unqualified signal via-in-pad fabrication.

    Same-net copper contacts may be legal, but an open drill must not intersect
    a solderable SMD land. GND return vias retain their separately allowed policy.
    """
    assigned = {p:n.name for n in board.nets for p in n.pads}
    for placement in board.placements:
        for pad in board.footprints[placement.footprint].pads:
            if pad.kind is not PadKind.SMD:
                continue
            net = assigned.get(PadReference(placement.reference,pad.number))
            for via in pair:
                if net != via.net:
                    continue
                shape = placed_pad_shape(transformed_local_point(placement,pad.position),pad,placement)
                if not shapes_clear(shape,RoundedConvexShape((via.position,),via.drill_nm//2),0):
                    return False
    return True


def _circle_sites(first: Point, first_radius: int, second: Point,
                  second_radius: int) -> tuple[tuple[float, float], ...]:
    """Continuous intersections; rounded sites are only proposals, never proof."""
    dx, dy = second.x_nm-first.x_nm, second.y_nm-first.y_nm
    distance = hypot(dx, dy)
    if (distance == 0 or distance > first_radius+second_radius
            or distance < abs(first_radius-second_radius)):
        return ()
    along = (first_radius*first_radius-second_radius*second_radius
             + distance*distance)/(2*distance)
    height_squared = first_radius*first_radius-along*along
    if height_squared < 0:
        # Floating cancellation at a tangent must not authorize geometry.
        if height_squared < -1:
            return ()
        height_squared = 0
    height = sqrt(height_squared)
    x, y = first.x_nm+along*dx/distance, first.y_nm+along*dy/distance
    return ((x-height*dy/distance, y+height*dx/distance),
            (x+height*dy/distance, y-height*dx/distance))


def _via_separation(board: PhysicalBoard, returned: Via, signal: Via,
                    clearance: int) -> int:
    """Copper, drill-to-copper and drill spacing, using actual via sizes."""
    hole = board.rules.minimum_hole_clearance_nm
    return max(returned.size_nm//2+signal.size_nm//2+clearance,
               returned.drill_nm//2+signal.drill_nm//2+hole,
               returned.drill_nm//2+signal.size_nm//2+hole,
               returned.size_nm//2+signal.drill_nm//2+hole)


def _return_positions(board: PhysicalBoard, port: _Port, pair: tuple[Via, Via],
                      first: NetRoutingRule, second: NetRoutingRule,
                      net: str, limit: int) -> tuple[Point, ...]:
    """Replace provably futile old slots with bounded actual circle/ring sites.

    The original unique axial/lattice count is the allowance. Preserve every
    old site inside both proximity disks and outside mandatory fabrication
    exclusions. Derived sites spend only slots that could not satisfy those
    scalar conditions. Net/region-specific clearance and exact all-layer
    geometry remain the fresh index's responsibility.
    """
    dx, dy = _HEADS[port.heading]
    legacy = []
    for distance in (.8, 1, 1.25, 1.5, 2):
        amount = nm_from_mm(distance)
        for sign in (1, -1):
            legacy.append(Point(port.center.x_nm+sign*dx*amount,
                                port.center.y_nm+sign*dy*amount))
    step = nm_from_mm(.125)
    legacy.extend(Point(port.center.x_nm+x*step,port.center.y_nm+y*step)
        for x,y in sorted(product(range(-16,17),repeat=2),key=lambda p:(
            p[0]*p[0]+p[1]*p[1],-(p[0]*dx+p[1]*dy),p)) if x or y)
    legacy = tuple(dict.fromkeys(legacy))
    returned = replace(pair[0], net=net)
    required = tuple(_via_separation(board, returned, signal,
        board.rules.minimum_clearance_nm) for signal in pair)

    def scalar_legal(position: Point) -> bool:
        distances = tuple((position.x_nm-v.position.x_nm)**2
                          +(position.y_nm-v.position.y_nm)**2 for v in pair)
        return all(minimum*minimum <= distance <= limit*limit
                   for minimum,distance in zip(required,distances))

    retained = tuple(position for position in legacy if scalar_legal(position))
    allowance = len(legacy)-len(retained)
    if allowance == 0:
        return retained
    return_rule = next((r for r in board.net_routing_rules if r.net == net), None)
    return_clearance = return_rule.clearance_nm or 0 if return_rule else 0
    radii = []
    for signal,rule in zip(pair,(first,second)):
        minimum = _via_separation(board, returned, signal, max(
            board.rules.minimum_clearance_nm, rule.clearance_nm or 0,
            return_clearance))
        radii.append(tuple(dict.fromkeys(radius for radius in (
            (minimum+limit)//2, minimum+4, limit-4, minimum, limit)
            if minimum <= radius <= limit)))
    sites = []
    for a,b in product(*radii):
        sites.extend(_circle_sites(pair[0].position,a,pair[1].position,b))
    # Circle tangencies along the actual pair axes also cover wide/unequal
    # rings, rather than assuming the port's nominal heading or equal vias.
    vx = pair[1].position.x_nm-pair[0].position.x_nm
    vy = pair[1].position.y_nm-pair[0].position.y_nm
    length = hypot(vx,vy)
    if length:
        axes = ((vx/length,vy/length),(-vy/length,vx/length))
        for signal,ring in zip(pair,radii):
            for radius,axis,sign in product(ring,axes,(1,-1)):
                sites.append((signal.position.x_nm+sign*radius*axis[0],
                              signal.position.y_nm+sign*radius*axis[1]))
    derived = []
    seen = set(retained)
    # One-nanometre neighbours handle integer rounding at a true tangent;
    # scalar and exact geometry checks still decide acceptance.
    nudges = ((0,0),(1,0),(-1,0),(0,1),(0,-1),(1,1),(1,-1),(-1,1),(-1,-1))
    for x,y in sites:
        for nx,ny in nudges:
            position = Point(round(x)+nx,round(y)+ny)
            if position in seen or not scalar_legal(position):
                continue
            seen.add(position)
            derived.append(position)
            if len(derived) == allowance:
                break
        if len(derived) == allowance:
            break
    # Keep the established axial preferences, then exact boundary proposals,
    # then every still-possible old lattice point. No new search allowance.
    axial = set(legacy[:10])
    return (*tuple(p for p in retained if p in axial), *derived,
            *tuple(p for p in retained if p not in axial))


def _return_via(board: PhysicalBoard, index: RoutingClearanceIndex,
                port: _Port, pair: tuple[Via, Via], first: NetRoutingRule,
                second: NetRoutingRule, surface: CopperLayer,
                layer: CopperLayer, *, trace=None, completion=None) -> tuple[Via, ...] | None:
    if not (first.require_return_vias or second.require_return_vias):
        return () if completion is None or completion(()) else None
    span = physical_via_span(board,surface,layer)
    if (span is None or tuple(v.net for v in pair) != (first.net,second.net)
            or any((v.from_layer,v.to_layer,v.technology) != span
                   or v.finish != "standard" for v in pair)
            or any(surface not in routing_layers(board,r.net,r)
                   or layer not in routing_layers(board,r.net,r)
                   for r in (first,second))):
        if trace is not None:
            trace.record('return_invalid_signal_contract')
        return None
    # The caller may reserve additional collars/vias, but not use an index
    # bound to old source, rules, placements or missing occurrence geometry.
    if (any(getattr(index.board,f.name) != getattr(board,f.name)
                for f in fields(board) if f.name not in {"tracks","vias"})
            or not Counter(index.board.tracks) >= Counter(board.tracks)
            or not Counter(index.board.vias) >= Counter(board.vias)
            or not index.candidate_vias_clear(pair)):
        if trace is not None:
            trace.record('return_invalid_index_or_signal_geometry')
        return None
    if shared_reference_plane(board, first, second, (surface, layer),
                              tuple(via.position for via in pair)) is not None:
        return () if completion is None or completion(()) else None
    requested = [rule for rule in (first, second) if rule.require_return_vias]
    if len({rule.return_via_net for rule in requested}) != 1:
        return None
    net = requested[0].return_via_net
    if net not in {item.name for item in board.nets}:
        return None
    limit = min(rule.maximum_return_via_distance_nm for rule in requested)
    for position in _return_positions(board,port,pair,first,second,net,limit):
        if trace is not None:
            trace.counts['return_sites_sampled'] += 1
        via = replace(pair[0], net=net, position=position)
        if (via_inside_board(board, position, via.size_nm)
                and index.candidate_vias_clear((*pair,via))):
            if completion is None or completion((via,)):
                return (via,)
            if trace is not None:
                trace.record('return_support_completion_rejected', positions=(position,))
            continue
        if trace is not None:
            trace.record('return_site_blocked', positions=(position,), index=index, vias=(via,))
    return None


def _local_via_ports(board, first, second, a, b, component, surface, layer, *, trace=None):
    """Finite recessed-land fallback: choose legal vias before searching collars.

    Surface-only portals can sit above opposite-face bypass pads or ground
    vias. Searching their lead-ins first hides the usable sites farther out.
    The stagger follows the terminal pitch, with 16 legal sites and 32 local
    path calls at most. Every path keeps both lanes, holes and old copper as
    obstacles; the normal transition and owner gates still run afterwards.
    """
    from .pin_access import local_access_path, LocalAccessStats
    span = physical_via_span(board, surface, layer)
    if span is None:
        return ()
    width = first.width_nm or board.rules.default_track_width_nm
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0,
                    second.clearance_nm or 0)
    offset = (transition_spacing(board, first, second)+1)//2
    middle = Point((a.x_nm+b.x_nm)//2, (a.y_nm+b.y_nm)//2)
    pitch = max(abs(a.x_nm-b.x_nm), abs(a.y_nm-b.y_nm))
    pose = next(p for p in board.placements if p.reference == component)
    outward = Point(middle.x_nm-pose.position.x_nm, middle.y_nm-pose.position.y_nm)
    headings = sorted((0, 2, 4, 6), key=lambda h:
        (-(_HEADS[h][0]*outward.x_nm+_HEADS[h][1]*outward.y_nm), h))
    results = []
    sites = calls = 0
    for heading, length, transverse, sign in product(headings,
            (.5, .75, 1, 1.5, 2, 3, 4, 4.5, 5, 6),
            (0, -pitch//2, pitch//2, -pitch, pitch), (-1, 1)):
        if sites >= 16 or calls >= 32 or len(results) >= 8:
            break
        dx, dy = _HEADS[heading]
        distance = nm_from_mm(length)
        center = Point(middle.x_nm+dx*distance-dy*transverse,
                       middle.y_nm+dy*distance+dx*transverse)
        normal = _normal(heading, offset)
        targets = (_shift(center, normal, sign), _shift(center, normal, -sign))
        pair = tuple(Via(rule.net, point, board.rules.default_via_size_nm,
                         board.rules.default_via_drill_nm, *span)
                     for rule, point in zip((first, second), targets))
        if trace is not None:
            trace.counts['local_sites_sampled'] += 1
        if not _signal_drills_outside_smd(board, pair):
            if trace is not None:
                trace.record('local_signal_drill_in_smd', positions=targets)
            continue
        if not _reserve_vias(board, RoutingClearanceIndex(board), pair):
            if trace is not None:
                trace.record('local_via_clearance', positions=targets,
                    index=RoutingClearanceIndex(board), vias=pair)
            continue
        sites += 1
        if trace is not None:
            trace.counts['local_legal_via_sites'] += 1
        approaches = tuple(Point(p.x_nm-dx*125000, p.y_nm-dy*125000) for p in targets)
        tails = tuple(_tracks(rule.net, (approach, target), width, surface,
                              RoutingClearanceIndex(board).breakout)
                      for rule, approach, target in zip((first, second), approaches, targets))
        if not _legal(board, RoutingClearanceIndex(board), *tails, clearance):
            if trace is not None:
                trace.record('local_tail_clearance', positions=targets,
                    index=RoutingClearanceIndex(board), tracks=(*tails[0], *tails[1]))
            continue
        for order in ((0, 1), (1, 0)):
            held = replace(board, tracks=(*board.tracks, *tails[0], *tails[1]),
                           vias=(*board.vias, *pair))
            index = _PairAccessIndex(held)
            paths = [None, None]
            for member in order:
                if calls >= 32:
                    break
                calls += 1
                local = LocalAccessStats() if trace is not None else None
                path = local_access_path(held, index, (first.net, second.net)[member],
                    (a, b)[member], approaches[member], surface, width,
                    step_nm=25000, detour_nm=500000, maximum_states=2400, stats=local)
                if trace is not None:
                    trace.record('local_path_' + local.outcome,
                        positions=((a, b)[member], approaches[member]), local=local)
                    trace.counts[f'local_{local.outcome}_member{member}_heading{heading}_sign{sign}'] += 1
                if path is None:
                    break
                paths[member] = (*path, *tails[member])
                for track in path:
                    index.add_track(track, locked=True)
            if (all(p is not None for p in paths)
                    and _legal(board, RoutingClearanceIndex(held), *paths, clearance)):
                results.append(_Port(center, heading, sign, *paths, 8))
                if trace is not None:
                    trace.counts['local_collars'] += 1
                break
    if trace is not None:
        trace.counts['local_path_calls'] += calls
        trace.counts['local_site_limit_reached'] += int(sites >= 16)
        trace.counts['local_call_limit_reached'] += int(calls >= 32)
    return tuple(results)


def _transitions(board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
                 first_position: Point, second_position: Point, component: str,
                 surface: CopperLayer, layer: CopperLayer,
                 site_rank: Callable[[tuple[Via, ...]], int | None] | None = None,
                 *, include_outer: bool = False, trace=None, terminal_completion=None,
                 stats: PairSearchStats | None = None,
                 ) -> tuple[_Transition, ...]:
    if surface is layer:
        # An opposite-face pair may use this terminal's own signal layer as
        # its middle layer. Keep a direct legal collar and charge no transition.
        width = first.width_nm or board.rules.default_track_width_nm
        offset = (width + first.pair_gap_nm + 1) // 2
        clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0,
                        second.clearance_nm or 0)
        index = RoutingClearanceIndex(board)
        results = []
        for family in range(2 if include_outer else 1):
            extra = ({"lengths": (4, 4.5, 5, 6), "allow_local_fallback": False}
                     if family else {})
            results.extend(_Transition(port, (), (), outer=bool(family)) for port in
                _ports(board, index, first.net, second.net, first_position, second_position,
                       component, width, offset, clearance, surface, **extra))
        if terminal_completion is not None:
            results = [replace(item, port=completed) for item in results
                if (completed := _complete_support_port(board, component,
                    item.port, terminal_completion)) is not None]
        domains = {}
        for item in results:
            domains.setdefault((item.port.heading, item.port.sign), {False: [], True: []})[item.outer].append(item)
        selected = {id(item) for families in domains.values()
                    for item in _interleave((families[False], families[True]), 16)}
        return tuple(item for item in results if id(item) in selected)
    span = physical_via_span(board, surface, layer)
    if span is None:
        return ()
    width = first.width_nm or board.rules.default_track_width_nm
    offset = (width + first.pair_gap_nm + 1) // 2
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0,
                    second.clearance_nm or 0)
    index = RoutingClearanceIndex(board)
    wide = _ports(board, index, first.net, second.net, first_position, second_position,
                  component, width, (transition_spacing(board, first, second)+1)//2,
                  clearance, surface)
    results = []
    rejections = Counter()
    # Local callers retain short-collar-first fallback. Joint channel search
    # also samples four outer collars beyond an opposite-side pad bank, even
    # when legal short collars exist but lead into isolated routing pockets.
    # These remain explicit source-profiled tracks, not wider breakout regions.
    # No local maze allowance is added by this fallback.
    for family in range(3):
        if family == 1:
            if results and not include_outer:
                break
            wide = _ports(board, index, first.net, second.net, first_position, second_position,
                          component, width, (transition_spacing(board, first, second)+1)//2,
                          clearance, surface, lengths=(4, 4.5, 5, 6), allow_local_fallback=False)
        elif family == 2:
            if results:
                break
            wide = _local_via_ports(board, first, second, first_position,
                                    second_position, component, surface, layer, trace=trace)
        if trace is not None:
            trace.counts[('short', 'outer', 'local')[family] + '_surface_collars'] += len(wide)
        for collar in wide:
            rejections['collars'] += 1
            pair = tuple(Via(net, tracks[-1].end, board.rules.default_via_size_nm,
                             board.rules.default_via_drill_nm, *span)
                         for net, tracks in ((first.net, collar.first), (second.net, collar.second)))
            if not _signal_drills_outside_smd(board,pair):
                if trace is not None:
                    trace.record('signal_drill_in_smd', positions=tuple(v.position for v in pair))
                rejections['drill_over_smd'] += 1
                continue
            escaped = replace(board, tracks=(*board.tracks, *collar.first, *collar.second))
            reserved = RoutingClearanceIndex(escaped)
            if not _reserve_vias(board, reserved, pair):
                if trace is not None:
                    trace.record('signal_via_clearance', positions=tuple(v.position for v in pair),
                        index=RoutingClearanceIndex(escaped), vias=pair)
                rejections['signal_vias_blocked'] += 1
                continue
            branches = ((), ())
            def complete_return(returns):
                nonlocal branches
                held = replace(escaped, vias=(*board.vias, *pair, *returns))
                proposed = terminal_completion(component, held,
                    collar.first, collar.second, "transition")
                if proposed is None:
                    return False
                branches = proposed
                return True
            returns = _return_via(board, reserved, collar, pair, first, second, surface, layer,
                trace=trace, completion=complete_return if terminal_completion is not None else None)
            if returns is None:
                if trace is not None:
                    trace.record('return_unavailable', positions=tuple(v.position for v in pair))
                rejections['return_via_blocked'] += 1
                continue
            # A caller's site rank may drop a transition (``None``) or order it
            # ahead of others; it never admits a site the checks above refused.
            rank = 0 if site_rank is None else site_rank((*pair, *returns))
            if rank is None:
                rejections['site_rank_rejected'] += 1
                continue
            escaped = replace(escaped, tracks=(*escaped.tracks, *branches[0], *branches[1]),
                              vias=(*board.vias, *pair, *returns))
            inner_ports = _ports(escaped, RoutingClearanceIndex(escaped), first.net, second.net,
                                pair[0].position, pair[1].position, component,
                                width, offset, clearance, layer)
            if trace is not None:
                trace.counts['middle_layer_collars'] += len(inner_ports)
                if not inner_ports:
                    trace.record('middle_layer_blocked', positions=tuple(v.position for v in pair))
            if not inner_ports:
                rejections['onward_collars_blocked'] += 1
            for inner in inner_ports:
                results.append(_Transition(replace(inner,
                    first=(*branches[0], *collar.first, *inner.first),
                    second=(*branches[1], *collar.second, *inner.second),
                    preference=collar.preference+inner.preference), pair, returns, rank, bool(family)))
    if not results and stats is not None:
        message = (f"{component} {surface.value} to {layer.value}: no exact transition; "
                   + ', '.join(f'{key}={count}' for key, count in sorted(rejections.items())))
        stats.terminal_port_rejections = tuple(dict.fromkeys((
            *stats.terminal_port_rejections, message)))[:4]
    # Preserve orientation diversity, then short collars; domain bound is
    # independent of maze expansion and never mutates previously locked copper.
    ordered = sorted(results, key=lambda t: (t.rank, t.port.preference,
        sum(hypot(s.end.x_nm-s.start.x_nm, s.end.y_nm-s.start.y_nm)
            for s in (*t.port.first, *t.port.second)), t.port.center.x_nm,
        t.port.center.y_nm))
    domains = {}
    for item in ordered:
        domains.setdefault((item.port.heading, item.port.sign), {False: [], True: []})[item.outer].append(item)
    selected = set()
    for families in domains.values():
        # The same 16 slots per heading/sign cover both short and outer sites.
        # An absent family lends its slots to the other; no domain grows.
        for item in _interleave((families[False], families[True]), 16):
            selected.add(id(item))
    return tuple(item for item in ordered if id(item) in selected)


def _interleave(groups, limit):
    """Stable family diversity inside an existing finite candidate allowance."""
    iterators = [iter(group) for group in groups]
    emitted = 0
    while iterators and emitted < limit:
        remaining = []
        for iterator in iterators:
            item = next(iterator, None)
            if item is None:
                continue
            yield item
            emitted += 1
            if emitted == limit:
                return
            remaining.append(iterator)
        iterators = remaining


def _facing_turns(first, second):
    def turns(port, target):
        dx, dy = target.center.x_nm-port.center.x_nm, target.center.y_nm-port.center.y_nm
        desired = max(range(8), key=lambda h: (_HEADS[h][0]*dx+_HEADS[h][1]*dy)/hypot(*_HEADS[h]))
        return min((port.heading-desired) % 8, (desired-port.heading) % 8)
    return turns(first, second)+turns(second, first)


def _cramped(first, second, pitch_nm):
    """Facing ports nearer along their headings than their side step needs.

    A 45-degree side step across the headings takes as much travel along
    them, plus a search step out of one port and into the other. Nearer
    than that, the pair has to turn back on itself, so its route is a loop.
    Long collars that reach toward each other look close, yet are exactly
    this.
    """
    if (first.heading - second.heading) % 8 != 4:
        return False
    dx, dy = _HEADS[first.heading]
    vx, vy = second.center.x_nm-first.center.x_nm, second.center.y_nm-first.center.y_nm
    # Both projections are scaled by the heading's length; so is the margin.
    return vx*dx+vy*dy < abs(vx*dy-vy*dx)+2*pitch_nm*hypot(dx, dy)


def _transition_pairs(groups, departure_rank=None, pitch_nm=0):
    families = {(False, False): [], (True, True): [], (False, True): [], (True, False): []}
    for a, b in product(*groups):
        if a.port.sign == -b.port.sign:
            families[a.outer, b.outer].append((a, b))
    def rank(pair):
        a, b = pair
        return (a.rank+b.rank,
                (departure_rank(a)+departure_rank(b)) if departure_rank is not None else 0,
                _facing_turns(a.port, b.port),
                _cramped(a.port, b.port, pitch_nm),
                a.port.preference+b.port.preference,
                hypot(a.port.center.x_nm-b.port.center.x_nm, a.port.center.y_nm-b.port.center.y_nm),
                a.port.center.x_nm, a.port.center.y_nm, b.port.center.x_nm, b.port.center.y_nm)
    ordered = tuple(sorted(group, key=rank) for group in families.values())
    return sum(map(len, ordered)), tuple(_interleave(ordered, 128))


def paired_via_candidates(board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
                          first_guide: GlobalNetRoute, second_guide: GlobalNetRoute, *,
                          maximum_searches: int = 8, maximum_states: int = 30_000,
                          pitch_nm: int = nm_from_mm(1),
                          stats: PairSearchStats | None = None,
                          maximum_total_states: int | None = None,
                          layers: tuple[CopperLayer, ...] | None = None,
                          site_rank: Callable[[tuple[Via, ...]], int | None] | None = None,
                          terminal_completion=None,
                          ) -> Iterator[PairSearchCandidate]:
    """Propose matched terminal transitions; owner checks DRC and profiles.

    ``layers`` restricts the layer between the transitions (a planned
    crossing, plan R5). ``site_rank`` receives each transition's signal and
    return vias and returns ``None`` to drop it or a rank, lower first.
    """
    if min(maximum_searches, maximum_states, pitch_nm) <= 0:
        raise ValueError("paired via search bounds must be positive")
    if maximum_total_states is not None and maximum_total_states <= 0:
        raise ValueError("paired via aggregate state bound must be positive")
    stats = stats if stats is not None else PairSearchStats()
    if maximum_total_states is not None and stats.expanded_states >= maximum_total_states:
        return
    width = first.width_nm or board.rules.default_track_width_nm
    if ((second.width_nm or board.rules.default_track_width_nm) != width
            or first.pair_gap_nm is None or second.pair_gap_nm != first.pair_gap_nm
            or len(first_guide.accesses) != 2 or len(second_guide.accesses) != 2):
        return
    from .pair_terminals import terminal_partners
    partners = terminal_partners(first_guide, second_guide)
    accesses = sorted(first_guide.accesses, key=lambda a: a.pad.component)
    if partners is None:
        return
    from .pair_terminals import terminal_copper_layers
    domains = tuple(terminal_copper_layers(board, a.pad).intersection(
        terminal_copper_layers(board, partners[a.pad].pad)) for a in accesses)
    allowed = set(routing_layers(board, first.net, first)) & set(routing_layers(board, second.net, second))
    domains = tuple(domain.intersection(allowed) for domain in domains)
    if any(not domain for domain in domains):
        return
    ranks, _ = signal_layer_preferences(board)
    common = set.intersection(*(set(domain) for domain in domains))
    # The primary same-layer search owns the first common contact layer.
    # Remaining layers include direct PTH contacts as zero-transition ends.
    target_layers = allowed - ({min(common, key=board.stackup.copper_layers.index)} if common else set())
    targets = sorted((layer for layer in target_layers
        if all(rule.max_vias is None or rule.max_vias >= sum(layer not in domain for domain in domains)
               for rule in (first, second))),
        key=lambda layer: (sum(layer not in domain for domain in domains), ranks[layer],
                           board.stackup.copper_layers.index(layer)))
    if layers is not None:
        targets = [layer for layer in targets if layer in layers]
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0, second.clearance_nm or 0)
    offset = (width+first.pair_gap_nm+1)//2
    for layer in targets:
        if not _remaining_states(stats, maximum_states, maximum_total_states):
            break
        surfaces = tuple(layer if layer in domain else min(domain, key=board.stackup.copper_layers.index)
                         for domain in domains)
        traces = tuple(_TransitionTrace() for _ in accesses)
        groups = tuple(_transitions(board, first, second, a.pad_position,
            partners[a.pad].pad_position, a.pad.component, surface, layer, site_rank,
            include_outer=True, stats=stats, trace=trace, terminal_completion=terminal_completion)
            for a, surface, trace in zip(accesses, surfaces, traces))
        for access, surface, group, trace in zip(accesses, surfaces, groups, traces):
            if len(stats.transition_inventory) < 24:
                stats.transition_inventory.append(PairTransitionInventory(access.pad.component,
                    surface, layer, pitch_nm, len(group), sum(not item.vias for item in group),
                    tuple(sorted(trace.counts.items())), tuple(trace.examples)))
        departure_ranks = {}
        def departure_rank(item):
            # A legal collar can face straight into opposite-face copper.
            # Sample its existing one-pitch straight continuation on the exact
            # local proposal before ranking pairs. This is only a preference:
            # a shorter legal goal tail may still reach a non-departable end.
            key = id(item)
            if key not in departure_ranks:
                held = replace(board, tracks=(*board.tracks, *item.port.first, *item.port.second),
                               vias=(*board.vias, *item.vias, *item.returns))
                departure_ranks[key] = int(not _can_depart(held, RoutingClearanceIndex(held),
                    first.net, second.net, item.port, width, offset, clearance, layer, pitch_nm))
            return departure_ranks[key]
        total_pairs, combinations = _transition_pairs(groups, departure_rank, pitch_nm)
        if not total_pairs:
            domains_text = "; ".join(
                f"{access.pad.component} {surface.value} to {layer.value}: "
                f"{len(group)} exact transitions, lane orders "
                f"{','.join(str(sign) for sign in sorted({t.port.sign for t in group})) or 'none'}"
                for access, surface, group in zip(accesses, surfaces, groups))
            stats.terminal_port_rejections = tuple(dict.fromkeys((
                *stats.terminal_port_rejections, domains_text)))[:4]
        stats.port_pairs += total_pairs
        searches_at_entry = stats.searches
        for ordinal,(a,b) in enumerate(combinations[:128],1):
            if stats.searches-searches_at_entry >= maximum_searches:
                break
            budget = _remaining_states(stats, maximum_states, maximum_total_states)
            if not budget:
                break
            start,end = a.port,b.port
            first_escapes=(*start.first,*end.first)
            second_escapes=(*start.second,*end.second)
            if not _legal(board, RoutingClearanceIndex(board), first_escapes, second_escapes, clearance):
                continue
            escaped = replace(board, tracks=(*board.tracks,*first_escapes,*second_escapes))
            vias = (*a.vias,*b.vias,*a.returns,*b.returns)
            if not _reserve_vias(board, RoutingClearanceIndex(escaped), vias):
                continue
            escaped=replace(escaped,vias=(*board.vias,*vias))
            escaped_index = RoutingClearanceIndex(escaped)
            if not _can_depart(escaped, escaped_index, first.net, second.net,
                               start, width, offset, clearance, layer, pitch_nm):
                dx, dy = _HEADS[start.heading]
                ahead = Point(start.center.x_nm+dx*pitch_nm, start.center.y_nm+dy*pitch_nm)
                normal = _normal(start.heading,offset)
                stats.reject_launch(escaped_index, (*_tracks(first.net,
                    (_shift(start.center,normal,start.sign),_shift(ahead,normal,start.sign)),width,layer,escaped_index.breakout),
                    *_tracks(second.net,(_shift(start.center,normal,-start.sign),_shift(ahead,normal,-start.sign)),width,layer,escaped_index.breakout)))
                continue
            stats.searches += 1
            candidate = _search(escaped, escaped_index, first.net, second.net,
                start,end,width,offset,clearance,layer,pitch_nm,budget,stats)
            from .pair_search_resume import register_pair_search
            via_pairs = tuple(pair for pair in (a.vias, b.vias) if pair)
            register_pair_search(stats, board, via_pairs=via_pairs,
                                 return_vias=(*a.returns, *b.returns),
                                 retain_frontier=maximum_total_states is not None)
            if candidate is None:
                continue
            p,q,expanded,_ = candidate
            stats.candidates += 1
            yield PairSearchCandidate(
                (*start.first,*p,*(replace(t,start=t.end,end=t.start) for t in reversed(end.first))),
                (*start.second,*q,*(replace(t,start=t.end,end=t.start) for t in reversed(end.second))),
                expanded,ordinal,via_pairs=via_pairs,return_vias=(*a.returns,*b.returns))
