"""Board-bound, directional escape-channel estimates for physical placement.

This is a conservative soft objective, not a pin-access or DRC certificate.
Cached local geometry never crosses board/rule snapshots or includes XY poses.
"""
from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, replace
from decimal import Decimal
from enum import Enum
from types import MappingProxyType
from typing import Mapping

from .geometry import Bounds
from .physical import (BoardSide, CopperLayer, PadKind, PhysicalBoard, Placement,
                       Point, RelativePlacementKind, nm_from_mm)
from .routing_layers import routing_layers


class EscapeSide(str, Enum):
    NORTH = "north"
    EAST = "east"
    SOUTH = "south"
    WEST = "west"


@dataclass(frozen=True, slots=True)
class EscapeDemand:
    pad_count: int = 0
    via_pad_count: int = 0
    bank_rows: int = 0
    depth_nm: int = 0
    width_nm: int = 0
    clearance_nm: int = 0


@dataclass(frozen=True, slots=True)
class PackageEscapeProfile:
    reference: str
    bounds: Bounds
    sides: Mapping[EscapeSide, EscapeDemand]

    def __post_init__(self):
        object.__setattr__(self, "sides", MappingProxyType(dict(self.sides)))


@dataclass(frozen=True, slots=True)
class EscapeChannel:
    left: str
    right: str
    axis: str
    left_side: EscapeSide
    right_side: EscapeSide
    gap_nm: int
    required_gap_nm: int
    left_depth_nm: int
    right_depth_nm: int
    left_rows: int
    right_rows: int
    pad_count: int

    @property
    def deficit_nm(self) -> int:
        return max(0, self.required_gap_nm - self.gap_nm)


def placement_units(board: PhysicalBoard) -> dict[str, frozenset[str]]:
    """Transitive rigid/proximity units; callers still apply every hard gate."""
    members = {p.reference: {p.reference} for p in board.placements}
    for group in (*({m.reference for m in c.members} for c in board.rigid_clusters),
                  *({t.reference for t in r.targets} for r in board.relative_rules
                    if r.kind is RelativePlacementKind.MAX_DISTANCE)):
        group &= members.keys()
        for reference in group:
            members[reference].update(group)
    for reference in members:
        unit = members[reference]
        while any(not members[item] <= unit for item in tuple(unit)):
            unit.update(*(members[item] for item in tuple(unit)))
        for item in unit:
            members[item] = unit
    return {reference: frozenset(unit) for reference, unit in members.items()}


class EscapeSpacingModel:
    """Reusable geometry/demand model for one immutable physical source board."""

    def __init__(self, board: PhysicalBoard, *, margin_nm: int = nm_from_mm("0.5"),
                 transit_lanes: int = 1):
        if margin_nm < 0 or transit_lanes < 0:
            raise ValueError("escape margin and transit lanes must be non-negative")
        self.board = board
        self.margin_nm = margin_nm
        self.transit_lanes = transit_lanes
        self.units = placement_units(board)
        self.rules = {r.net: r for r in board.net_routing_rules}
        zone_nets = {z.net for z in board.zones}
        self.net_by_pad = {(p.component, p.pad): n.name for n in board.nets
                           if len(n.pads) >= 2 or n.name in zone_nets for p in n.pads}
        self.layers = {n.name: routing_layers(board, n.name, self.rules.get(n.name))
                       for n in board.nets}
        self.pad_counts = {p.reference: len({land.number for land in board.footprints[p.footprint].pads
                          if land.number and land.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}})
                           for p in board.placements}
        self.rigid_pairs = {frozenset((a.reference, b.reference)) for c in board.rigid_clusters
                            for a in c.members for b in c.members if a.reference != b.reference}
        self._profiles: dict[tuple[str, str, BoardSide, Decimal], PackageEscapeProfile] = {}
        # Keep only each reference/pair's most recent pose, not an unbounded
        # memo of every placement trial. One-component refinements can reuse
        # all unaffected pairs without recalculating their envelopes/demands.
        self._positioned = {}
        self._pairs = {}

    def profile(self, placement: Placement) -> PackageEscapeProfile:
        key = (placement.reference, placement.footprint, placement.side, placement.rotation_degrees)
        if key in self._profiles:
            return self._profiles[key]
        from .drc import placed_pad_shape
        from .placement import transformed_local_point, transformed_footprint_polygon

        local_pose = replace(placement, position=Point(0, 0))
        footprint = self.board.footprints[placement.footprint]
        lands = [(pad, placed_pad_shape(transformed_local_point(local_pose, pad.position), pad, local_pose).bounds)
                 for pad in footprint.pads if pad.number
                 and pad.kind not in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}]
        outline = transformed_footprint_polygon(self.board, local_pose)
        min_x, min_y = min(p.x_nm for p in outline), min(p.y_nm for p in outline)
        max_x, max_y = max(p.x_nm for p in outline), max(p.y_nm for p in outline)
        extent = Bounds(min([min_x, *(b.min_x for _, b in lands)]),
                        min([min_y, *(b.min_y for _, b in lands)]),
                        max([max_x, *(b.max_x for _, b in lands)]),
                        max([max_y, *(b.max_y for _, b in lands)]))
        active = defaultdict(list)
        surface = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        for pad, bounds in lands:
            net = self.net_by_pad.get((placement.reference, pad.number))
            if net is None or pad.kind is not PadKind.SMD:
                continue
            # Ordinary two-terminal passives block a neighbour's access but
            # do not demand their own via banks. Do not scatter local networks.
            if self.pad_counts[placement.reference] <= 2:
                continue
            distances = {EscapeSide.NORTH: bounds.min_y - extent.min_y,
                         EscapeSide.EAST: extent.max_x - bounds.max_x,
                         EscapeSide.SOUTH: extent.max_y - bounds.max_y,
                         EscapeSide.WEST: bounds.min_x - extent.min_x}
            side = min(distances, key=lambda s: (distances[s], len(active[s]), s.value))
            rule = self.rules.get(net)
            width = max(self.board.rules.minimum_track_width_nm,
                        rule.width_nm if rule and rule.width_nm else self.board.rules.default_track_width_nm)
            clearance = max(self.board.rules.minimum_clearance_nm,
                            rule.clearance_nm if rule and rule.clearance_nm else 0)
            allowed = self.layers[net]
            via = (not rule or rule.max_vias != 0) and any(layer != surface for layer in allowed)
            active[side].append((bounds, via, width, clearance))
        sides = {}
        for side in EscapeSide:
            pads = active[side]
            if not pads:
                sides[side] = EscapeDemand()
                continue
            width = max(p[2] for p in pads)
            clearance = max(p[3] for p in pads)
            via_pads = [p[0] for p in pads if p[1]]
            rows = 0
            if via_pads:
                horizontal = side in {EscapeSide.NORTH, EscapeSide.SOUTH}
                span = (max(b.max_x for b in via_pads) - min(b.min_x for b in via_pads) if horizontal
                        else max(b.max_y for b in via_pads) - min(b.min_y for b in via_pads))
                via_pitch = max(self.board.rules.default_via_size_nm + clearance,
                                self.board.rules.default_via_drill_nm + self.board.rules.minimum_hole_clearance_nm)
                facade = extent.max_x - extent.min_x if horizontal else extent.max_y - extent.min_y
                # Permit bounded lateral staggering beyond the assigned pad
                # window, never more than the available package-side span.
                span = min(facade, span + 2 * via_pitch)
                # Available centres must fit complete annuli inside the span.
                slots = max(1, 1 + max(0, span - self.board.rules.default_via_size_nm) // via_pitch)
                rows = (len(via_pads) + slots - 1) // slots
                depth = self.board.rules.default_via_size_nm + (rows - 1) * via_pitch
            else:
                depth = width  # Surface-only signals still need a launch lane.
            sides[side] = EscapeDemand(len(pads), len(via_pads), rows,
                                       depth + clearance + self.margin_nm, width, clearance)
        profile = PackageEscapeProfile(placement.reference, extent, sides)
        self._profiles[key] = profile
        return profile

    def channels(self, placements: Mapping[str, Placement], *, include_clear: bool = False) -> tuple[EscapeChannel, ...]:
        positioned = []
        for reference in sorted(placements):
            placement = placements[reference]
            key = (placement.footprint, placement.side, placement.rotation_degrees, placement.position)
            cached = self._positioned.get(reference)
            if cached is None or cached[0] != key:
                profile = self.profile(placement)
                b, p = profile.bounds, placement.position
                cached = (key, profile, Bounds(b.min_x+p.x_nm, b.min_y+p.y_nm,
                                               b.max_x+p.x_nm, b.max_y+p.y_nm))
                self._positioned[reference] = cached
            positioned.append((placement, *cached))
        result = []
        for index, (left, lk, lp, lb) in enumerate(positioned):
            for right, rk, rp, rb in positioned[index + 1:]:
                pair_key = (left.reference, right.reference, include_clear)
                cached = self._pairs.get(pair_key)
                if cached is not None and cached[:2] == (lk, rk):
                    result.extend(cached[2])
                    continue
                pair = []
                if left.side is not right.side or frozenset((left.reference, right.reference)) in self.rigid_pairs:
                    self._pairs[pair_key] = (lk, rk, ())
                    continue
                if (right.reference in self.units[left.reference]
                        and min(self.pad_counts[left.reference], self.pad_counts[right.reference]) <= 2):
                    self._pairs[pair_key] = (lk, rk, ())
                    continue  # A close two-terminal companion travels with its owner.
                if min(lb.max_y, rb.max_y) > max(lb.min_y, rb.min_y):
                    east = right.position.x_nm >= left.position.x_nm
                    pair.extend(self._channel(left, right, lp, rp, "x",
                        EscapeSide.EAST if east else EscapeSide.WEST,
                        EscapeSide.WEST if east else EscapeSide.EAST,
                        rb.min_x-lb.max_x if east else lb.min_x-rb.max_x, include_clear))
                if min(lb.max_x, rb.max_x) > max(lb.min_x, rb.min_x):
                    south = right.position.y_nm >= left.position.y_nm
                    pair.extend(self._channel(left, right, lp, rp, "y",
                        EscapeSide.SOUTH if south else EscapeSide.NORTH,
                        EscapeSide.NORTH if south else EscapeSide.SOUTH,
                        rb.min_y-lb.max_y if south else lb.min_y-rb.max_y, include_clear))
                self._pairs[pair_key] = (lk, rk, tuple(pair))
                result.extend(pair)
        return tuple(result)

    def _channel(self, left, right, lp, rp, axis, ls, rs, gap, include_clear):
        a, b = lp.sides[ls], rp.sides[rs]
        if not a.pad_count and not b.pad_count:
            return ()
        width = max(a.width_nm, b.width_nm, self.board.rules.default_track_width_nm)
        clearance = max(a.clearance_nm, b.clearance_nm, self.board.rules.minimum_clearance_nm)
        transit = (self.transit_lanes * width + (self.transit_lanes + 1) * clearance
                   if self.transit_lanes else clearance if a.depth_nm and b.depth_nm else 0)
        required = a.depth_nm + b.depth_nm + transit
        if not include_clear and gap >= required:
            return ()
        return (EscapeChannel(left.reference, right.reference, axis, ls, rs, gap, required,
                              a.depth_nm, b.depth_nm, a.bank_rows, b.bank_rows,
                              a.pad_count + b.pad_count),)
