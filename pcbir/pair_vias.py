"""Bounded paired terminal transitions plus a joint single-layer middle route.

Short surface collars widen to manufacturing-legal matched through-vias;
the other signal layer tapers back to the declared pair spacing. Both lanes
are searched together. This is not arbitrary 3-D maze routing or SI signoff.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from itertools import product
from math import hypot
from typing import Iterator

from .pair_search import (PairSearchCandidate, PairSearchStats, _Port, _HEADS,
                          _legal, _ports, _search, _remaining_states)
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


@dataclass(frozen=True, slots=True)
class _Transition:
    port: _Port
    vias: tuple[Via, Via]
    returns: tuple[Via, ...]


def transition_spacing(board: PhysicalBoard, first: NetRoutingRule,
                       second: NetRoutingRule) -> int:
    """Via pads/drills need wider spacing than the coupled trace pitch."""
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0,
                    second.clearance_nm or 0)
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


def _return_via(board: PhysicalBoard, index: RoutingClearanceIndex,
                port: _Port, pair: tuple[Via, Via], first: NetRoutingRule,
                second: NetRoutingRule) -> tuple[Via, ...] | None:
    if not (first.require_return_vias or second.require_return_vias):
        return ()
    requested = [rule for rule in (first, second) if rule.require_return_vias]
    if len({rule.return_via_net for rule in requested}) != 1:
        return None
    net = requested[0].return_via_net
    if net not in {item.name for item in board.nets}:
        return None
    limit = min(rule.maximum_return_via_distance_nm for rule in requested)
    dx, dy = _HEADS[port.heading]
    # Symmetric forward/backward reference-via positions, not between the
    # signal vias (which would collide). No hard-coded component coordinates.
    positions = []
    for distance in (.8, 1, 1.25, 1.5, 2):
        amount = nm_from_mm(distance)
        for sign in (1, -1):
            positions.append(Point(port.center.x_nm + sign * dx * amount,
                                   port.center.y_nm + sign * dy * amount))
    # Forward/backward slots alone miss legal lateral reference sites between
    # package lands. Try a bounded local lattice too; original distance and
    # all-layer copper/drill constraints still apply to every candidate.
    step = nm_from_mm(.125)
    positions.extend(Point(port.center.x_nm+x*step,port.center.y_nm+y*step)
        for x,y in sorted(product(range(-16,17),repeat=2),key=lambda p:(
            p[0]*p[0]+p[1]*p[1],-(p[0]*dx+p[1]*dy),p)) if x or y)
    for position in dict.fromkeys(positions):
        if any(hypot(position.x_nm-v.position.x_nm,
                     position.y_nm-v.position.y_nm) > limit for v in pair):
            continue
        via = replace(pair[0], net=net, position=position)
        if (via_inside_board(board, position, via.size_nm)
                and index.can_via(net, position, via.size_nm, via.from_layer,
                                  via.to_layer, via.drill_nm, check_hole_copper=True)):
            return (via,)
    return None


def _transitions(board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
                 first_position: Point, second_position: Point, component: str,
                 surface: CopperLayer, layer: CopperLayer) -> tuple[_Transition, ...]:
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
        returns = _return_via(board, reserved, collar, pair, first, second)
        if returns is None:
            continue
        escaped = replace(escaped, vias=(*board.vias, *pair, *returns))
        for inner in _ports(escaped, RoutingClearanceIndex(escaped), first.net, second.net,
                            pair[0].position, pair[1].position, component,
                            width, offset, clearance, layer):
            results.append(_Transition(replace(inner,
                first=(*collar.first, *inner.first), second=(*collar.second, *inner.second),
                preference=collar.preference+inner.preference), pair, returns))
    # Preserve orientation diversity, then short collars; domain bound is
    # independent of maze expansion and never mutates previously locked copper.
    ordered = sorted(results, key=lambda t: (t.port.preference,
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
                          maximum_total_states: int | None = None) -> Iterator[PairSearchCandidate]:
    """Propose two matched transitions/member; owner checks DRC and profiles."""
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
    partners = {a.pad.component: a for a in second_guide.accesses}
    accesses = sorted(first_guide.accesses, key=lambda a: a.pad.component)
    if len(partners) != 2 or set(partners) != {a.pad.component for a in accesses}:
        return
    placements = {p.reference:p for p in board.placements}
    surfaces = tuple(CopperLayer.FRONT if placements[a.pad.component].side is BoardSide.FRONT
                     else CopperLayer.BACK for a in accesses)
    allowed = set(routing_layers(board, first.net, first)) & set(routing_layers(board, second.net, second))
    if any(surface not in allowed for surface in surfaces):
        return
    ranks, _ = signal_layer_preferences(board)
    targets = sorted(allowed-set(surfaces), key=lambda layer: (ranks[layer], board.stackup.copper_layers.index(layer)))
    clearance = max(board.rules.minimum_clearance_nm, first.clearance_nm or 0, second.clearance_nm or 0)
    offset = (width+first.pair_gap_nm+1)//2
    for layer in targets:
        if not _remaining_states(stats, maximum_states, maximum_total_states):
            break
        groups = tuple(_transitions(board, first, second, a.pad_position,
            partners[a.pad.component].pad_position, a.pad.component, surface, layer)
            for a, surface in zip(accesses, surfaces))
        combinations = sorted(((a,b) for a,b in product(*groups) if a.port.sign == -b.port.sign),
            key=lambda pair: (pair[0].port.preference+pair[1].port.preference,
                hypot(pair[0].port.center.x_nm-pair[1].port.center.x_nm,
                      pair[0].port.center.y_nm-pair[1].port.center.y_nm),
                pair[0].port.center.x_nm, pair[0].port.center.y_nm,
                pair[1].port.center.x_nm, pair[1].port.center.y_nm))
        stats.port_pairs += len(combinations)
        for ordinal,(a,b) in enumerate(combinations[:maximum_searches],1):
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
            stats.searches += 1
            candidate = _search(escaped, RoutingClearanceIndex(escaped), first.net, second.net,
                start,end,width,offset,clearance,layer,pitch_nm,budget,stats)
            if candidate is None:
                continue
            p,q,expanded,_ = candidate
            stats.candidates += 1
            yield PairSearchCandidate(
                (*start.first,*p,*(replace(t,start=t.end,end=t.start) for t in reversed(end.first))),
                (*start.second,*q,*(replace(t,start=t.end,end=t.start) for t in reversed(end.second))),
                expanded,ordinal,via_pairs=(a.vias,b.vias),return_vias=(*a.returns,*b.returns))
