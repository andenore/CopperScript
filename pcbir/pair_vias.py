"""Bounded paired terminal transitions plus a joint single-layer middle route.

Short surface collars widen to manufacturing-legal matched through-vias;
the other signal layer tapers back to the declared pair spacing. Both lanes
are searched together. This is not arbitrary 3-D maze routing or SI signoff.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass, fields, replace
from itertools import product
from math import hypot, sqrt
from typing import Callable, Iterator

from .pair_search import (PairSearchCandidate, PairSearchStats, _Port, _HEADS,
                          _legal, _ports, _search, _remaining_states, _can_depart)
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
    vias: tuple[Via, Via]
    returns: tuple[Via, ...]
    # Caller's site preference (lower first); 0 unless a site rank is given.
    rank: int = 0


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
                layer: CopperLayer) -> tuple[Via, ...] | None:
    if not (first.require_return_vias or second.require_return_vias):
        return ()
    span = physical_via_span(board,surface,layer)
    if (span is None or tuple(v.net for v in pair) != (first.net,second.net)
            or any((v.from_layer,v.to_layer,v.technology) != span
                   or v.finish != "standard" for v in pair)
            or any(surface not in routing_layers(board,r.net,r)
                   or layer not in routing_layers(board,r.net,r)
                   for r in (first,second))):
        return None
    # The caller may reserve additional collars/vias, but not use an index
    # bound to old source, rules, placements or missing occurrence geometry.
    if (any(getattr(index.board,f.name) != getattr(board,f.name)
                for f in fields(board) if f.name not in {"tracks","vias"})
            or not Counter(index.board.tracks) >= Counter(board.tracks)
            or not Counter(index.board.vias) >= Counter(board.vias)
            or not index.candidate_vias_clear(pair)):
        return None
    if shared_reference_plane(board, first, second, (surface, layer),
                              tuple(via.position for via in pair)) is not None:
        return ()
    requested = [rule for rule in (first, second) if rule.require_return_vias]
    if len({rule.return_via_net for rule in requested}) != 1:
        return None
    net = requested[0].return_via_net
    if net not in {item.name for item in board.nets}:
        return None
    limit = min(rule.maximum_return_via_distance_nm for rule in requested)
    for position in _return_positions(board,port,pair,first,second,net,limit):
        via = replace(pair[0], net=net, position=position)
        if (via_inside_board(board, position, via.size_nm)
                and index.candidate_vias_clear((*pair,via))):
            return (via,)
    return None


def _transitions(board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
                 first_position: Point, second_position: Point, component: str,
                 surface: CopperLayer, layer: CopperLayer,
                 site_rank: Callable[[tuple[Via, ...]], int | None] | None = None,
                 ) -> tuple[_Transition, ...]:
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
    # Short collars retain priority. If all are rejected by actual transition
    # geometry, try four bounded longer collars beyond a back-side pad bank.
    # These remain explicit source-profiled tracks, not wider breakout regions.
    # No local maze allowance is added by this fallback.
    for family in range(2):
        if family:
            if results:
                break
            wide = _ports(board, index, first.net, second.net, first_position, second_position,
                          component, width, (transition_spacing(board, first, second)+1)//2,
                          clearance, surface, lengths=(4, 4.5, 5, 6), allow_local_fallback=False)
        for collar in wide:
            pair = tuple(Via(net, tracks[-1].end, board.rules.default_via_size_nm,
                             board.rules.default_via_drill_nm, *span)
                         for net, tracks in ((first.net, collar.first), (second.net, collar.second)))
            if not _signal_drills_outside_smd(board,pair):
                continue
            escaped = replace(board, tracks=(*board.tracks, *collar.first, *collar.second))
            reserved = RoutingClearanceIndex(escaped)
            if not _reserve_vias(board, reserved, pair):
                continue
            returns = _return_via(board, reserved, collar, pair, first, second, surface, layer)
            if returns is None:
                continue
            # A caller's site rank may drop a transition (``None``) or order it
            # ahead of others; it never admits a site the checks above refused.
            rank = 0 if site_rank is None else site_rank((*pair, *returns))
            if rank is None:
                continue
            escaped = replace(escaped, vias=(*board.vias, *pair, *returns))
            for inner in _ports(escaped, RoutingClearanceIndex(escaped), first.net, second.net,
                                pair[0].position, pair[1].position, component,
                                width, offset, clearance, layer):
                results.append(_Transition(replace(inner,
                    first=(*collar.first, *inner.first), second=(*collar.second, *inner.second),
                    preference=collar.preference+inner.preference), pair, returns, rank))
    # Preserve orientation diversity, then short collars; domain bound is
    # independent of maze expansion and never mutates previously locked copper.
    ordered = sorted(results, key=lambda t: (t.rank, t.port.preference,
        sum(hypot(s.end.x_nm-s.start.x_nm, s.end.y_nm-s.start.y_nm)
            for s in (*t.port.first, *t.port.second)), t.port.center.x_nm,
        t.port.center.y_nm))
    counts = {}
    retained = []
    for item in ordered:
        key = item.port.heading, item.port.sign
        if counts.get(key, 0) < 16:
            retained.append(item)
            counts[key] = counts.get(key, 0) + 1
    return tuple(retained)


def paired_via_candidates(board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
                          first_guide: GlobalNetRoute, second_guide: GlobalNetRoute, *,
                          maximum_searches: int = 8, maximum_states: int = 30_000,
                          pitch_nm: int = nm_from_mm(1),
                          stats: PairSearchStats | None = None,
                          maximum_total_states: int | None = None,
                          layers: tuple[CopperLayer, ...] | None = None,
                          site_rank: Callable[[tuple[Via, ...]], int | None] | None = None,
                          ) -> Iterator[PairSearchCandidate]:
    """Propose two matched transitions/member; owner checks DRC and profiles.

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
            or any(rule.max_vias is not None and rule.max_vias < 2 for rule in (first, second))
            or len(first_guide.accesses) != 2 or len(second_guide.accesses) != 2):
        return
    from .pair_terminals import terminal_partners
    partners = terminal_partners(first_guide, second_guide)
    accesses = sorted(first_guide.accesses, key=lambda a: a.pad.component)
    if partners is None:
        return
    placements = {p.reference:p for p in board.placements}
    surfaces = tuple(CopperLayer.FRONT if placements[a.pad.component].side is BoardSide.FRONT
                     else CopperLayer.BACK for a in accesses)
    allowed = set(routing_layers(board, first.net, first)) & set(routing_layers(board, second.net, second))
    if any(surface not in allowed for surface in surfaces):
        return
    ranks, _ = signal_layer_preferences(board)
    targets = sorted(allowed-set(surfaces), key=lambda layer: (ranks[layer], board.stackup.copper_layers.index(layer)))
    if layers is not None:
        targets = [layer for layer in targets if layer in layers]
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0, second.clearance_nm or 0)
    offset = (width+first.pair_gap_nm+1)//2
    for layer in targets:
        if not _remaining_states(stats, maximum_states, maximum_total_states):
            break
        groups = tuple(_transitions(board, first, second, a.pad_position,
            partners[a.pad].pad_position, a.pad.component, surface, layer, site_rank)
            for a, surface in zip(accesses, surfaces))
        combinations = sorted(((a,b) for a,b in product(*groups) if a.port.sign == -b.port.sign),
            key=lambda pair: (pair[0].port.preference+pair[1].port.preference,
                hypot(pair[0].port.center.x_nm-pair[1].port.center.x_nm,
                      pair[0].port.center.y_nm-pair[1].port.center.y_nm),
                pair[0].port.center.x_nm, pair[0].port.center.y_nm,
                pair[1].port.center.x_nm, pair[1].port.center.y_nm))
        stats.port_pairs += len(combinations)
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
                continue
            stats.searches += 1
            candidate = _search(escaped, escaped_index, first.net, second.net,
                start,end,width,offset,clearance,layer,pitch_nm,budget,stats)
            from .pair_search_resume import register_pair_search
            register_pair_search(stats, board, via_pairs=(a.vias, b.vias),
                                 return_vias=(*a.returns, *b.returns),
                                 retain_frontier=maximum_total_states is not None)
            if candidate is None:
                continue
            p,q,expanded,_ = candidate
            stats.candidates += 1
            yield PairSearchCandidate(
                (*start.first,*p,*(replace(t,start=t.end,end=t.start) for t in reversed(end.first))),
                (*start.second,*q,*(replace(t,start=t.end,end=t.start) for t in reversed(end.second))),
                expanded,ordinal,via_pairs=(a.vias,b.vias),return_vias=(*a.returns,*b.returns))
