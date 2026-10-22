"""Escape planning: which side, layer and order package pins leave by.

Step E0 of the escape planning plan (docs/escape-planning-plan.md) is
diagnostics only. For the placed board it reports, per component:

* **channel load**: signal pins inside the pad field's outer ring must pass
  between ring pads; demand is their count, capacity the tracks the ring's
  gaps hold on the pins' own copper layers, ⌊(gap − s)/(w + s)⌋ per gap;
* **two-end crossings** for each *cluster pair* (two components joined by
  at least ``cluster_pair_minimum_nets`` signal nets): each net's pin is
  ordered along its component's boundary, cut on the side facing away from
  the other component. Two nets whose orders disagree cross between the two
  parts on one layer. The largest non-crossing subset is a longest
  non-decreasing subsequence (Ozdal & Wong); the remaining nets need another
  layer or a detour around a part.

Step E1 adds the channel model the escape solvers build on: each component's
pads split into *pad fields* by spacing; a field is a lattice (BGA, header),
a row, a perimeter (QFN, SOIC) or irregular, with the gaps between
neighbouring pins and, for a lattice, the tiles between them with their
diagonal gaps (Yan & Wong) and via sites. Capacities use each net's escape
rule, necked down where its breakout rule says so.

Step E3 plans crossing bundles (``plan_two_end_escapes``): the largest
non-crossing subset stays on one layer, the rest move to another through a
via near the surface end, and the bundle routes first. Fanout and the
ordinary detailed router follow the plan only when ``two_end_order`` is set.

Diagnostics never change routing. Every number is computed from integer pad
geometry with deterministic tie-breaks.
"""
from __future__ import annotations

from bisect import bisect_right
from collections.abc import Mapping
from dataclasses import dataclass
from math import atan2, isqrt, pi
from statistics import median_low
from types import MappingProxyType

from .drc import placed_pad_shape
from .geometry import (Bounds, RoundedConvexShape, bounds, orientation, point_on_segment,
                       shape_distance_squared)
from .physical import (BoardSide, CopperLayer, NetRoutingRule, PadKind, PadReference, PhysicalBoard, Point)
from .placement import transformed_local_point
from .routing_costs import LayerBias
from .routing_layers import routing_layers
from .zone_geometry import distribution_zone_nets


@dataclass(frozen=True, slots=True)
class EscapePlanOptions:
    # Two components sharing this many signal nets are planned as a pair.
    cluster_pair_minimum_nets: int = 2
    # Pairs with more shared nets are reported without an order analysis.
    maximum_pair_nets: int = 256
    # Pads of one component closer than this many times their median neighbour
    # distance, or than the minimum, form one pad field.
    field_spacing_factor: int = 3
    field_spacing_minimum_nm: int = 3_000_000
    # Two-end planning (E3): plan crossing bundles' layers and route order.
    two_end_order: bool = False
    # The corridor of a bundle is its pins' box grown by this much.
    corridor_margin_nm: int = 1_000_000
    # Extra cost per millimetre for a moved net off its planned layer in the
    # corridor, in routing cost units (10 doubles the cost of track). Weaker
    # biases leave a moved net on the planar layer, where it takes the gap a
    # kept net needs (D3 0049 and led-ring lose nets at 0, 5 and 10); the
    # price is a via where a planar detour existed.
    layer_bias_rate: int = 20
    # Array escape (E2): plan surface escapes of ball grids by min-cost flow.
    array_flow: bool = False
    # Bus layers (E5): spread a crossing bundle's moved nets over successive
    # layers, each a largest non-crossing subset, within each net's via budget.
    bus_layers: bool = False


@dataclass(frozen=True, slots=True)
class ClusterPin:
    pad: PadReference
    net: str | None
    position: Point
    layers: tuple[CopperLayer, ...]
    outer: bool


@dataclass(frozen=True, slots=True)
class ChannelLoad:
    """Signal pins inside a pad field's outer ring against the tracks its gaps hold."""

    layer: CopperLayer
    demand: int
    capacity: int
    gaps: int

    @property
    def overflow(self) -> int:
        return max(0, self.demand - self.capacity)


@dataclass(frozen=True, slots=True)
class ClusterEscape:
    component: str
    pins: tuple[ClusterPin, ...]
    bounds: Bounds
    inner_signal_pins: tuple[PadReference, ...]
    channels: tuple[ChannelLoad, ...]
    fields: tuple[PadField, ...] = ()
    # Escape track width and clearance of the component's signal nets (the widest), for field reports.
    rules: tuple[int, int] = (0, 0)

    @property
    def overflow(self) -> int:
        """Inner signal pins the ring gaps cannot pass: surface pins on their layer, through-hole pins on any."""
        spare = {channel.layer: channel.capacity for channel in self.channels}
        layers = {pin.pad: pin.layers for pin in self.pins}
        short = 0
        for layer in spare:
            surface = sum(layers[pad] == (layer,) for pad in self.inner_signal_pins)
            short += max(0, surface - spare[layer])
            spare[layer] = max(0, spare[layer] - surface)
        through = sum(len(layers[pad]) > 1 for pad in self.inner_signal_pins)
        return short + max(0, through - sum(spare.values()))

    def report(self) -> dict:
        return {"component": self.component, "pins": len(self.pins),
                "inner_signal_pins": [_pad_text(pad) for pad in self.inner_signal_pins],
                "channels": [{"layer": item.layer.value, "demand": item.demand, "capacity": item.capacity,
                              "gaps": item.gaps, "overflow": item.overflow} for item in self.channels],
                "overflow": self.overflow,
                "fields": [field.report(*self.rules) for field in self.fields]}


@dataclass(frozen=True, slots=True)
class ClusterPairEscape:
    first: str
    second: str
    nets: tuple[str, ...]
    # Net pairs whose boundary orders disagree: they cross on one layer.
    crossing_pairs: int
    crossing_nets: tuple[str, ...]
    # Nets outside one largest non-crossing subset: another layer or a detour.
    layer_moves: tuple[str, ...]
    analysed: bool = True
    # The bundle along the first component's boundary, from its far side.
    order: tuple[str, ...] = ()
    # Each net of ``order``'s place along the second component's boundary.
    second_keys: tuple[tuple, ...] = ()

    def report(self) -> dict:
        return {"components": [self.first, self.second], "nets": list(self.nets),
                "crossing_pairs": self.crossing_pairs, "crossing_nets": list(self.crossing_nets),
                "layer_moves": list(self.layer_moves), "analysed": self.analysed}


@dataclass(frozen=True, slots=True)
class EscapeDiagnostics:
    clusters: tuple[ClusterEscape, ...]
    pairs: tuple[ClusterPairEscape, ...]

    def report(self) -> dict:
        """Clusters with a channel load or overflow, and every cluster pair."""
        return {"schema": "copperscript.escape-diagnostics/v0.1",
                "clusters": [item.report() for item in self.clusters if item.inner_signal_pins],
                "pairs": [item.report() for item in self.pairs],
                "overflow_clusters": sorted(item.component for item in self.clusters if item.overflow),
                "crossing_pairs": sum(item.crossing_pairs for item in self.pairs)}

    def net_causes(self) -> dict[str, list[str]]:
        """Escape causes per net: ``two_end_crossing`` and ``channel_overflow``.

        Every net of a bundle with a crossing is tagged, not only the crossing
        ones: whichever nets route first take the gaps the others need.
        """
        causes: dict[str, set[str]] = {}
        for pair in self.pairs:
            for net in pair.nets if pair.crossing_pairs else ():
                causes.setdefault(net, set()).add("two_end_crossing")
        pin_nets = {pin.pad: pin.net for cluster in self.clusters for pin in cluster.pins}
        for cluster in self.clusters:
            if cluster.overflow:
                for pad in cluster.inner_signal_pins:
                    causes.setdefault(pin_nets[pad], set()).add("channel_overflow")
        return {net: sorted(values) for net, values in sorted(causes.items())}


def escape_diagnostics(board: PhysicalBoard, options: EscapePlanOptions | None = None) -> EscapeDiagnostics:
    options = options or EscapePlanOptions()
    if options.cluster_pair_minimum_nets < 1 or options.maximum_pair_nets < 1:
        raise ValueError("escape plan bounds must be positive")
    zone_nets = distribution_zone_nets(board)
    pad_nets = {pad: net.name for net in board.nets if len(net.pads) >= 2 for pad in net.pads}
    signal = {net for net in pad_nets.values() if net not in zone_nets}
    clusters = tuple(cluster for placement in sorted(board.placements, key=lambda item: item.reference)
                     if (cluster := _cluster(board, placement, pad_nets, signal, options)) is not None)
    by_component = {cluster.component: cluster for cluster in clusters}
    # A bundle is the signal nets whose pads all lie on the two components;
    # nets reaching further parts are trees, free to leave the pair elsewhere.
    shared: dict[tuple[str, str], set[str]] = {}
    for net in board.nets:
        components = sorted({pad.component for pad in net.pads})
        if net.name in signal and len(components) == 2 and all(item in by_component for item in components):
            shared.setdefault((components[0], components[1]), set()).add(net.name)
    pairs = tuple(_pair(by_component[first], by_component[second], tuple(sorted(nets)), options)
                  for (first, second), nets in sorted(shared.items())
                  if len(nets) >= options.cluster_pair_minimum_nets)
    return EscapeDiagnostics(clusters, pairs)


# --------------------------------------------------------------------------- clusters

def _cluster(board: PhysicalBoard, placement, pad_nets: dict[PadReference, str],
             signal: set[str], options: EscapePlanOptions) -> ClusterEscape | None:
    placed = _placed_pads(board, placement)
    if not placed:
        return None
    hull = _hull([position for _pad, position, _layers, _shape in placed])
    pins = tuple(ClusterPin(PadReference(placement.reference, pad.number),
                            pad_nets.get(PadReference(placement.reference, pad.number)), position, layers,
                            _on_boundary(position, hull))
                 for pad, position, layers, _shape in placed)
    # A surface land wide enough for a via (an exposed pad) leaves through its own via, not the ring.
    via_lands = {PadReference(placement.reference, pad.number) for pad, _position, layers, _shape in placed
                 if len(layers) == 1 and min(pad.size.width_nm, pad.size.height_nm) >= board.rules.default_via_size_nm}
    inner = tuple(pin.pad for pin in pins if not pin.outer and pin.net in signal and pin.pad not in via_lands)
    channels = _channels(board, placement.reference, placed, pins, inner, pad_nets) if inner else ()
    nets = {pin.net for pin in pins if pin.net in signal}
    rules = _net_rules(board, nets) if nets else channel_rules(board, "")
    return ClusterEscape(placement.reference, pins, bounds(pin.position for pin in pins), inner, channels,
                         _pad_fields(board, placement, placed, options), rules)


def _channels(board: PhysicalBoard, reference: str, placed: list, pins: tuple[ClusterPin, ...],
              inner: tuple[PadReference, ...], pad_nets: dict[PadReference, str]) -> tuple[ChannelLoad, ...]:
    """Ring gaps on each layer the inner pins reach, against the inner pins there."""
    width, clearance = _net_rules(board, {pad_nets[pad] for pad in inner})
    centre = _centre([pin.position for pin in pins])
    ring = sorted((index for index, pin in enumerate(pins) if pin.outer),
                  key=lambda index: (_angle(pins[index].position, centre, 0.0), pins[index].pad))
    inner_layers = {pin.pad: pin.layers for pin in pins}
    result = []
    for layer in sorted({layer for pad in inner for layer in inner_layers[pad]}, key=_layer_rank):
        demand = sum(layer in inner_layers[pad] for pad in inner)
        on_layer = [index for index in ring if layer in pins[index].layers]
        capacity = gaps = 0
        if len(on_layer) >= 2:
            for position, index in enumerate(on_layer):
                following = on_layer[(position + 1) % len(on_layer)]
                if following == index or (len(on_layer) == 2 and position == 1):
                    continue
                gap = _edge_gap(placed[index][3], placed[following][3])
                capacity += max(0, (gap - clearance) // (width + clearance))
                gaps += 1
        result.append(ChannelLoad(layer, demand, capacity, gaps))
    return tuple(result)


def _net_rules(board: PhysicalBoard, nets: set[str]) -> tuple[int, int]:
    """The widest escape track and largest escape clearance among ``nets`` (see ``channel_rules``)."""
    rules = {rule.net: rule for rule in board.net_routing_rules}
    widths, clearances = zip(*(channel_rules(board, net, rules.get(net)) for net in sorted(nets)))
    return max(widths), max(clearances)


def channel_rules(board: PhysicalBoard, net: str, rule: NetRoutingRule | None = None) -> tuple[int, int]:
    """Track width and clearance of ``net`` between a pad field's pins.

    A net with breakout properties necks down there, as the routers do inside
    its terminal regions (``breakout.BreakoutRegions``); any other net uses its
    own rule, else the board default, never below the board minimums.
    """
    breakout = rule is not None and rule.breakout_length_nm is not None
    if breakout and rule.breakout_width_nm is not None:
        width = rule.breakout_width_nm
    else:
        width = max(board.rules.minimum_track_width_nm,
                    rule.width_nm if rule is not None and rule.width_nm else board.rules.default_track_width_nm)
    if breakout and rule.breakout_clearance_nm is not None:
        clearance = rule.breakout_clearance_nm
    else:
        clearance = (rule.clearance_nm or 0) if rule is not None else 0
    return width, max(board.rules.minimum_clearance_nm, clearance)


def _edge_gap(first, second) -> int:
    distance = isqrt(int(shape_distance_squared(first, second)))
    return max(0, distance - first.radius_nm - second.radius_nm)


# --------------------------------------------------------------------------- channel model (E1)

@dataclass(frozen=True, slots=True)
class Gap:
    """The copper gap between two neighbouring pins of a pad field, on the layers both lands occupy."""

    first: PadReference
    second: PadReference
    kind: str  # "row", "column" or "diagonal" in a lattice; "ring" along any other field's outline
    gap_nm: int
    layers: tuple[CopperLayer, ...]

    def capacity(self, width_nm: int, clearance_nm: int) -> int:
        """Tracks of ``width_nm`` side by side at ``clearance_nm``: ⌊(gap − s)/(w + s)⌋."""
        return max(0, (self.gap_nm - clearance_nm) // (width_nm + clearance_nm))


TILE_SIDES = ("W", "N", "E", "S")
TILE_CORNERS = ("NW", "NE", "SE", "SW")
# Each diagonal of a tile splits its terminals (sides, and corner pins off that
# diagonal) into two halves. A track joining the halves passes between the
# diagonal's two pins and uses that diagonal gap; one within a half does not.
_DIAGONAL_HALVES = (({"N", "E", "NE"}, {"W", "S", "SW"}),  # NW–SE
                    ({"W", "N", "NW"}, {"E", "S", "SE"}))  # NE–SW


@dataclass(frozen=True, slots=True)
class Tile:
    """The cell between four neighbouring lattice positions.

    Directions are in the footprint's own frame (x to the east, y to the
    south) before placement. A missing pin leaves its corner ``None`` and the
    sides and diagonal through it unbounded.
    """

    column: int
    row: int
    corners: tuple[PadReference | None, PadReference | None, PadReference | None, PadReference | None]
    sides: tuple[Gap | None, Gap | None, Gap | None, Gap | None]  # W, N, E, S
    diagonals: tuple[Gap | None, Gap | None]  # NW–SE, NE–SW
    centre: Point
    via_site: bool  # the board's default via fits at the centre, clear of every corner land

    def admits(self, flows: Mapping[tuple[str, str], int], layer: CopperLayer, width_nm: int,
               clearance_nm: int) -> bool:
        """Whether tracks between the given terminals fit through the tile on ``layer``.

        ``flows`` counts tracks per (terminal, terminal), each a side or a
        corner pin. Every side gap bounds the tracks through it, every pin
        starts at most one, and each diagonal gap bounds the tracks joining
        its two halves (Yan & Wong): counting tracks per tile instead loses
        tracks that turn round different corners, and ignoring diagonals
        admits tracks that do not fit between two diagonal pins.
        """
        use = dict.fromkeys(TILE_SIDES + TILE_CORNERS, 0)
        for (first, second), count in flows.items():
            if count < 0 or first == second or first not in use or second not in use:
                raise ValueError(f"invalid tile flow {first}->{second}: {count}")
            use[first] += count
            use[second] += count
        for index, corner in enumerate(TILE_CORNERS):
            if use[corner] > (1 if self.corners[index] is not None else 0):
                return False
        for index, side in enumerate(TILE_SIDES):
            gap = self.sides[index]
            if gap is not None and layer in gap.layers and use[side] > gap.capacity(width_nm, clearance_nm):
                return False
        for gap, (one, other) in zip(self.diagonals, _DIAGONAL_HALVES):
            if gap is None or layer not in gap.layers:
                continue
            crossing = sum(count for (first, second), count in flows.items()
                           if (first in one and second in other) or (first in other and second in one))
            if crossing > gap.capacity(width_nm, clearance_nm):
                return False
        return True


@dataclass(frozen=True, slots=True)
class PadField:
    """Pads of one component that escape together, with their channel graph."""

    component: str
    pads: tuple[PadReference, ...]
    kind: str  # "lattice", "row", "perimeter" or "irregular"
    columns: int  # lattice positions per row (0 unless a lattice)
    rows: int
    pitch_nm: tuple[int, int] | None  # lattice (x, y) pitch in the footprint frame
    gaps: tuple[Gap, ...]
    tiles: tuple[Tile, ...]

    def report(self, width_nm: int, clearance_nm: int) -> dict:
        """Counts and the narrowest gap of each kind, in tracks at the given rules."""
        kinds = sorted({gap.kind for gap in self.gaps})
        return {"pads": len(self.pads), "kind": self.kind,
                **({"columns": self.columns, "rows": self.rows, "pitch_nm": list(self.pitch_nm)}
                   if self.pitch_nm else {}),
                "gaps": {kind: sum(gap.kind == kind for gap in self.gaps) for kind in kinds},
                "minimum_capacity": {kind: min(gap.capacity(width_nm, clearance_nm) for gap in self.gaps
                                               if gap.kind == kind) for kind in kinds},
                "tiles": len(self.tiles), "via_sites": sum(tile.via_site for tile in self.tiles)}


def pad_fields(board: PhysicalBoard, component: str,
               options: EscapePlanOptions | None = None) -> tuple[PadField, ...]:
    """The pad fields of a placed component, largest first."""
    options = options or EscapePlanOptions()
    placement = next((item for item in board.placements if item.reference == component), None)
    if placement is None:
        raise ValueError(f"unknown component {component!r}")
    return _pad_fields(board, placement, _placed_pads(board, placement), options)


def _placed_pads(board: PhysicalBoard, placement) -> list[tuple]:
    """(pad, world position, copper layers, placed shape) of every copper pad."""
    surface = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    placed = []
    for pad in board.footprints[placement.footprint].pads:
        if pad.kind in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}:
            continue
        position = transformed_local_point(placement, pad.position)
        layers = (surface,) if pad.kind is PadKind.SMD else tuple(board.stackup.copper_layers)
        placed.append((pad, position, layers, placed_pad_shape(position, pad, placement)))
    return placed


def _pad_fields(board: PhysicalBoard, placement, placed: list[tuple],
                options: EscapePlanOptions) -> tuple[PadField, ...]:
    if options.field_spacing_factor < 1 or options.field_spacing_minimum_nm < 0:
        raise ValueError("pad field spacing must be positive")
    groups = _spacing_groups([item[0].position for item in placed], options)
    return tuple(_pad_field(board, placement, [placed[index] for index in group]) for group in groups)


def _spacing_groups(points: list[Point], options: EscapePlanOptions) -> list[list[int]]:
    """Single-linkage groups of footprint-local pad centres, largest first."""
    if len(points) <= 1:
        return [list(range(len(points)))]

    def squared(a: Point, b: Point) -> int:
        return (a.x_nm - b.x_nm) ** 2 + (a.y_nm - b.y_nm) ** 2

    nearest = [min(squared(point, other) for j, other in enumerate(points) if j != i)
               for i, point in enumerate(points)]
    link = max(options.field_spacing_factor * isqrt(median_low(nearest)), options.field_spacing_minimum_nm)
    parent = list(range(len(points)))

    def root(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i in range(len(points)):
        for j in range(i + 1, len(points)):
            if squared(points[i], points[j]) <= link * link:
                parent[root(i)] = root(j)
    groups: dict[int, list[int]] = {}
    for index in range(len(points)):
        groups.setdefault(root(index), []).append(index)
    return sorted(groups.values(), key=lambda group: (-len(group), group[0]))


def _pad_field(board: PhysicalBoard, placement, placed: list[tuple]) -> PadField:
    reference = placement.reference
    pads = tuple(PadReference(reference, item[0].number) for item in placed)
    lattice = _lattice([item[0].position for item in placed])
    if lattice is not None:
        xs, ys = lattice
        return _lattice_field(board, placement, placed, pads, xs, ys)
    hull = _hull([item[1] for item in placed])
    outer = [index for index, item in enumerate(placed) if _on_boundary(item[1], hull)]
    kind = "row" if len(hull) <= 2 else "perimeter" if len(outer) == len(placed) else "irregular"
    if kind == "row":  # neighbours along the line, which runs between the hull's two ends
        start = hull[0]
        ring = sorted(range(len(placed)), key=lambda index: ((placed[index][1].x_nm - start.x_nm) ** 2
                                                             + (placed[index][1].y_nm - start.y_nm) ** 2, pads[index]))
        links = list(zip(ring, ring[1:]))
    else:
        centre = _centre([item[1] for item in placed])
        ring = sorted(outer, key=lambda index: (_angle(placed[index][1], centre, 0.0), pads[index]))
        links = [(index, ring[(position + 1) % len(ring)]) for position, index in enumerate(ring)]
    gaps = [_gap(placed, pads, first, second, "ring") for first, second in links]
    return PadField(reference, pads, kind, 0, 0, None, tuple(gaps), ())


def _lattice(points: list[Point]) -> tuple[list[int], list[int]] | None:
    """Uniform footprint-frame columns and rows holding at least half their positions, else None."""
    xs = sorted({point.x_nm for point in points})
    ys = sorted({point.y_nm for point in points})
    if len(xs) < 2 or len(ys) < 2 or len({(p.x_nm, p.y_nm) for p in points}) != len(points):
        return None
    if 2 * len(points) < len(xs) * len(ys):
        return None
    for values in (xs, ys):
        steps = [b - a for a, b in zip(values, values[1:])]
        if max(steps) - min(steps) > _LATTICE_TOLERANCE_NM:
            return None
    return xs, ys


# Footprint coordinates within this of a uniform pitch still form a lattice.
_LATTICE_TOLERANCE_NM = 1_000


def _lattice_field(board: PhysicalBoard, placement, placed: list[tuple], pads: tuple[PadReference, ...],
                   xs: list[int], ys: list[int]) -> PadField:
    at = {(xs.index(item[0].position.x_nm), ys.index(item[0].position.y_nm)): index
          for index, item in enumerate(placed)}
    gaps: dict[tuple[tuple[int, int], tuple[int, int]], Gap] = {}
    for (column, row), index in sorted(at.items()):
        for step, kind in (((1, 0), "row"), ((0, 1), "column"), ((1, 1), "diagonal"), ((1, -1), "diagonal")):
            neighbour = (column + step[0], row + step[1])
            if neighbour in at:
                gaps[((column, row), neighbour)] = _gap(placed, pads, index, at[neighbour], kind)

    def gap(a: tuple[int, int], b: tuple[int, int]) -> Gap | None:
        return gaps.get((a, b)) or gaps.get((b, a))

    via_radius = board.rules.default_via_size_nm // 2
    tiles = []
    for column in range(len(xs) - 1):
        for row in range(len(ys) - 1):
            nw, ne, se, sw = (column, row), (column + 1, row), (column + 1, row + 1), (column, row + 1)
            corners = tuple(pads[at[spot]] if spot in at else None for spot in (nw, ne, se, sw))
            centre = transformed_local_point(placement, Point((xs[column] + xs[column + 1]) // 2,
                                                              (ys[row] + ys[row + 1]) // 2))
            point = RoundedConvexShape((centre,), 0)
            via_site = all(_edge_gap(point, placed[at[spot]][3]) >= via_radius + board.rules.minimum_clearance_nm
                           for spot in (nw, ne, se, sw) if spot in at)
            tiles.append(Tile(column, row, corners, (gap(nw, sw), gap(nw, ne), gap(ne, se), gap(sw, se)),
                              (gap(nw, se), gap(ne, sw)), centre, via_site))
    return PadField(placement.reference, pads, "lattice", len(xs), len(ys), (xs[1] - xs[0], ys[1] - ys[0]),
                    tuple(gaps[key] for key in sorted(gaps)), tuple(tiles))


def _gap(placed: list[tuple], pads: tuple[PadReference, ...], first: int, second: int, kind: str) -> Gap:
    layers = tuple(layer for layer in placed[first][2] if layer in placed[second][2])
    return Gap(pads[first], pads[second], kind, _edge_gap(placed[first][3], placed[second][3]), layers)


# --------------------------------------------------------------------------- pairs

def _pair(first: ClusterEscape, second: ClusterEscape, nets: tuple[str, ...],
          options: EscapePlanOptions) -> ClusterPairEscape:
    if len(nets) > options.maximum_pair_nets:
        return ClusterPairEscape(first.component, second.component, nets, 0, (), (), analysed=False)
    first_centre = _centre([pin.position for pin in first.pins])
    second_centre = _centre([pin.position for pin in second.pins])
    # Each boundary is cut on its far side; the second is read in the opposite
    # sense, so a crossing-free bundle has the same order at both ends.
    first_order = _boundary_order(first, nets, first_centre, second_centre, 1.0)
    second_order = _boundary_order(second, nets, second_centre, first_centre, -1.0)
    ranked = sorted(nets, key=lambda net: (first_order[net], second_order[net], net))
    crossing_nets: set[str] = set()
    crossings = 0
    for index, net in enumerate(ranked):
        for other in ranked[index + 1:]:
            if first_order[net] < first_order[other] and second_order[net] > second_order[other]:
                crossings += 1
                crossing_nets.update((net, other))
    kept = _longest_chain(ranked, second_order)
    return ClusterPairEscape(first.component, second.component, nets, crossings, tuple(sorted(crossing_nets)),
                             tuple(net for net in nets if net not in kept), order=tuple(ranked),
                             second_keys=tuple(second_order[net] for net in ranked))


def _boundary_order(cluster: ClusterEscape, nets: tuple[str, ...], centre: tuple[float, float],
                    far: tuple[float, float], sense: float) -> dict[str, tuple]:
    """Each net's place along the cluster boundary, starting on the side away from ``far``.

    Seen from the centre (inside the pins' convex hull), angle order is
    perimeter order. A single row has no inside: its pins would tie at two
    angles, so it is seen from just behind its centre, where the row is the
    face toward ``far``. A net with several pins in the cluster is placed at
    the pin nearest ``far``.
    """
    away = (centre[0] - far[0], centre[1] - far[1])
    length = (away[0] ** 2 + away[1] ** 2) ** 0.5 or 1.0
    away = (away[0] / length, away[1] / length)
    view = centre
    if len(_hull([pin.position for pin in cluster.pins])) <= 2:
        view = (centre[0] + away[0] * _BEHIND_NM, centre[1] + away[1] * _BEHIND_NM)
    cut = atan2(away[1], away[0])
    order: dict[str, tuple] = {}
    for pin in sorted(cluster.pins, key=lambda item: item.pad):
        if pin.net not in nets:
            continue
        reach = (pin.position.x_nm - view[0]) ** 2 + (pin.position.y_nm - view[1]) ** 2
        place = (_angle(pin.position, view, cut, sense), reach, pin.pad)
        distance = (pin.position.x_nm - far[0]) ** 2 + (pin.position.y_nm - far[1]) ** 2
        best = order.get(pin.net)
        if best is None or distance < best[0]:
            order[pin.net] = (distance, place)
    return {net: place for net, (_distance, place) in order.items()}


# A single row is seen from this far behind its centre.
_BEHIND_NM = 1_000_000


def _longest_chain(ranked: list[str], second_order: dict[str, tuple]) -> set[str]:
    """Nets of one longest subsequence of ``ranked`` that also increases in ``second_order``."""
    tails: list[tuple] = []
    tail_index: list[int] = []
    previous: list[int] = []
    for index, net in enumerate(ranked):
        value = second_order[net]
        slot = bisect_right(tails, value)
        if slot == len(tails):
            tails.append(value)
            tail_index.append(index)
        else:
            tails[slot] = value
            tail_index[slot] = index
        previous.append(tail_index[slot - 1] if slot else -1)
    kept: set[str] = set()
    index = tail_index[-1] if tail_index else -1
    while index >= 0:
        kept.add(ranked[index])
        index = previous[index]
    return kept


# --------------------------------------------------------------------------- two-end plan (E3)

@dataclass(frozen=True, slots=True)
class PairPlan:
    """Layer assignment and route order of one crossing bundle."""

    first: str
    second: str
    planar_layer: CopperLayer
    kept: tuple[str, ...]  # on the planar layer, in boundary order
    moved: tuple[tuple[str, CopperLayer], ...]  # to another layer through a via
    unmoved: tuple[str, ...]  # outside the planar subset, but with no other legal layer
    corridor: Bounds

    def report(self) -> dict:
        return {"components": [self.first, self.second], "planar_layer": self.planar_layer.value,
                "kept": list(self.kept), "moved": {net: layer.value for net, layer in self.moved},
                "unmoved": list(self.unmoved),
                "corridor_nm": [self.corridor.min_x, self.corridor.min_y, self.corridor.max_x, self.corridor.max_y]}


@dataclass(frozen=True, slots=True)
class EscapePlan:
    pairs: tuple[PairPlan, ...]
    # Nets to route first, in this order: each bundle's moved nets, then its kept nets outermost first.
    net_order: tuple[str, ...]
    layer_biases: tuple[LayerBias, ...]
    # Surface pins of kept nets: they escape on their surface, without a dogbone via.
    surface_pads: frozenset[PadReference] = frozenset()

    def report(self) -> dict:
        return {"pairs": [pair.report() for pair in self.pairs], "net_order": list(self.net_order),
                "surface_pads": sorted(_pad_text(pad) for pad in self.surface_pads)}


def plan_two_end_escapes(board: PhysicalBoard, options: EscapePlanOptions | None = None,
                         diagnostics: EscapeDiagnostics | None = None) -> EscapePlan:
    """Planar layer assignment and route order for every crossing bundle (Ozdal & Wong).

    A bundle whose two ends cross is planned together with every other signal
    net joining the same two parts. The largest non-crossing subset stays on the *planar layer*:
    the surface of a part whose bundle pins are all surface lands, else the
    first layer the nets share. Every other net moves to the legal layer
    nearest it in the stackup, changing layer through a via near the surface
    part. The assignment is soft: inside the bundle's corridor a moved net
    pays ``layer_bias_rate`` per millimetre on any layer but its own. Kept nets
    are not biased: they already start on the planar layer, and at a
    through-hole end every layer is theirs (holding them to the planar layer
    there squeezes the bundle through one layer's gaps). A bundle routes
    before other nets, moved nets first, then kept nets from the outside in,
    so no net takes the gap an outer one needs.
    """
    options = options or EscapePlanOptions()
    if options.corridor_margin_nm < 0 or options.layer_bias_rate < 0:
        raise ValueError("escape plan corridor margin and layer bias must not be negative")
    diagnostics = diagnostics or escape_diagnostics(board, options)
    clusters = {cluster.component: cluster for cluster in diagnostics.clusters}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    stack = tuple(board.stackup.copper_layers)
    pairs, order, biases, surface_pads = [], [], [], set()
    zone_nets = distribution_zone_nets(board)
    for crossing in diagnostics.pairs:
        if not crossing.analysed or not crossing.crossing_pairs:
            continue
        # The plan covers every signal net joining the two parts, rails and
        # other trees included: they cross the bundle too, and an unplanned
        # one routed last finds its lane taken.
        members = {pin.net for pin in clusters[crossing.first].pins if pin.net is not None} & {
            pin.net for pin in clusters[crossing.second].pins if pin.net is not None}
        nets = tuple(sorted(net for net in members if net not in zone_nets))
        pair = _pair(clusters[crossing.first], clusters[crossing.second], nets, options)
        if not pair.analysed:
            continue
        pins = [pin for component in (pair.first, pair.second) for pin in clusters[component].pins
                if pin.net in pair.nets]
        allowed = {net: routing_layers(board, net, rules.get(net)) for net in pair.nets}
        surfaces = {pin.layers[0] for pin in pins if len(pin.layers) == 1}
        shared = [layer for layer in stack if all(layer in layers for layers in allowed.values())]
        if len(surfaces) > 1 or not shared or surfaces and next(iter(surfaces)) not in shared:
            continue  # bundle pins on both outer layers, or no common layer: nothing planar to keep
        planar = next(iter(surfaces)) if surfaces else shared[0]
        moves = set(pair.layer_moves)
        kept = tuple(net for net in pair.order if net not in moves)
        moved, unmoved = [], []
        if options.bus_layers:
            moved, unmoved = _layered_moves(pair, kept, planar, allowed, rules, pins, stack)
        for net in pair.order if not options.bus_layers else ():
            if net not in moves:
                continue
            others = [layer for layer in allowed[net] if layer is not planar]
            if others:
                moved.append((net, min(others, key=lambda layer: abs(stack.index(layer) - stack.index(planar)))))
            else:
                unmoved.append(net)
        corridor = bounds(pin.position for pin in pins).expanded(options.corridor_margin_nm)
        plan = PairPlan(pair.first, pair.second, planar, kept, tuple(moved), tuple(unmoved), corridor)
        pairs.append(plan)
        groups = [[net for net, assigned in moved if assigned is layer] for layer in dict.fromkeys(l for _n, l in moved)]
        order.extend(net for net in [*(net for group in groups for net in _outside_in(group)), *_outside_in(kept),
                                     *unmoved] if net not in order)
        for net, layer in sorted(moved):
            biases.extend(LayerBias(net, other, corridor, options.layer_bias_rate)
                          for other in allowed[net] if other is not layer)
        # Only nets running purely between the two parts escape on the surface;
        # a rail or other tree keeps its dogbone towards its further pads.
        surface_pads.update(pin.pad for pin in pins
                            if pin.net in kept and pin.net in crossing.nets and pin.layers == (planar,))
    return EscapePlan(tuple(pairs), tuple(order), tuple(biases), frozenset(surface_pads))


def _outside_in(nets) -> list[str]:
    """A bundle's nets from both outer edges towards the middle."""
    nets = list(nets)
    return [nets[index // 2] if index % 2 == 0 else nets[-1 - index // 2] for index in range(len(nets))]


def _layered_moves(pair: ClusterPairEscape, kept, planar: CopperLayer, allowed, rules, pins, stack):
    """Moved nets over successive layers, each a largest non-crossing subset (Ozdal & Wong).

    Layers are tried nearest the planar layer first. A net joins a layer only
    if it may use it and its ``max_vias`` covers the layer changes at its
    surface lands (a through-hole land changes layer for free). Nets that fit
    no layer stay unmoved.
    """
    second = dict(zip(pair.order, pair.second_keys))
    remaining = [net for net in pair.order if net not in kept]
    surface_ends = {net: [pin.layers[0] for pin in pins if pin.net == net and len(pin.layers) == 1]
                    for net in remaining}

    def fits(net: str, layer: CopperLayer) -> bool:
        rule = rules.get(net)
        changes = sum(end is not layer for end in surface_ends[net])
        return layer in allowed[net] and (rule is None or rule.max_vias is None or changes <= rule.max_vias)

    moved = []
    for layer in sorted((layer for layer in stack if layer is not planar),
                        key=lambda layer: (abs(stack.index(layer) - stack.index(planar)), stack.index(layer))):
        candidates = [net for net in remaining if fits(net, layer)]
        chain = _longest_chain(candidates, second)
        moved.extend((net, layer) for net in candidates if net in chain)
        remaining = [net for net in remaining if net not in chain]
    return moved, remaining


# --------------------------------------------------------------------------- array escape (E2)

@dataclass(frozen=True, slots=True)
class ArrayEscape:
    """Which signal balls of a surface ball grid escape on its surface layer (min-cost max-flow).

    ``surface`` balls leave between the other balls on ``layer``; ``vias``
    are the signal balls the surface channels cannot take, which need a via.
    ``tile_load`` counts the planned surface tracks through each tile
    ((column, row) in the footprint frame); a via is best placed in a tile
    without load.
    """

    component: str
    layer: CopperLayer
    surface: tuple[PadReference, ...]
    vias: tuple[PadReference, ...]
    tile_load: Mapping[tuple[int, int], int]

    def report(self) -> dict:
        return {"component": self.component, "layer": self.layer.value, "surface": len(self.surface),
                "vias": len(self.vias), "loaded_tiles": sum(1 for load in self.tile_load.values() if load)}


# A side or diagonal through a missing pin is unbounded; this many tracks stands in for it.
_OPEN_CAPACITY = 1_000


def plan_array_escapes(board: PhysicalBoard, options: EscapePlanOptions | None = None) -> tuple[ArrayEscape, ...]:
    """Surface escapes of every surface-mount ball grid, by integer min-cost max-flow (Yan & Wong).

    Edge balls leave directly. An inner ball enters one of its four tiles;
    tracks cross from tile to tile through side gaps at the gap's capacity
    and leave the field through the outer gaps. A tile passes at most as many
    tracks as its narrower diagonal holds: exact for straight tracks and
    conservative for turning ones (Yan & Wong couple the two diagonals
    exactly; a plain flow cannot). Each tile step costs one, so the flow
    keeps the most balls on the surface and, among those, the shortest
    escapes. Plane (zone) balls never count: they take a via to their plane.
    """
    options = options or EscapePlanOptions()
    zone_nets = distribution_zone_nets(board)
    pad_nets = {pad: net.name for net in board.nets if len(net.pads) >= 2 for pad in net.pads}
    result = []
    for placement in sorted(board.placements, key=lambda item: item.reference):
        placed = _placed_pads(board, placement)
        for field in _pad_fields(board, placement, placed, options):
            if field.kind != "lattice" or not field.tiles:
                continue
            layers = {item[2] for item in placed if PadReference(placement.reference, item[0].number) in field.pads}
            if len(layers) != 1 or len(next(iter(layers))) != 1:
                continue  # through-hole or mixed fields escape on every layer
            layer = next(iter(layers))[0]
            signal = {pad for pad in field.pads if pad in pad_nets and pad_nets[pad] not in zone_nets}
            if not signal:
                continue
            width, clearance = _net_rules(board, {pad_nets[pad] for pad in signal})
            result.append(_array_flow(placement.reference, field, layer, signal, width, clearance))
    return tuple(result)


def _array_flow(component: str, field: "PadField", layer: CopperLayer, signal: set[PadReference],
                width: int, clearance: int) -> ArrayEscape:
    def capacity(gap) -> int:
        if gap is None or layer not in gap.layers:
            return _OPEN_CAPACITY
        return gap.capacity(width, clearance)

    tiles = {(tile.column, tile.row): tile for tile in field.tiles}
    columns, rows = field.columns, field.rows
    balls: dict[PadReference, tuple[int, int]] = {}
    for (column, row), tile in tiles.items():
        for corner, (dx, dy) in zip(tile.corners, ((0, 0), (1, 0), (1, 1), (0, 1))):
            if corner is not None:
                balls[corner] = (column + dx, row + dy)
    ordered_balls = sorted(pad for pad in balls if pad in signal)
    ordered_tiles = sorted(tiles)
    source, sink = 0, 1
    ball_node = {pad: 2 + index for index, pad in enumerate(ordered_balls)}
    tile_in = {key: 2 + len(ordered_balls) + 2 * index for index, key in enumerate(ordered_tiles)}
    flow = _MinCostFlow(2 + len(ordered_balls) + 2 * len(ordered_tiles))
    for pad in ordered_balls:
        column, row = balls[pad]
        node = ball_node[pad]
        flow.add(source, node, 1, 0)
        if column in (0, columns - 1) or row in (0, rows - 1):
            flow.add(node, sink, 1, 0)
        for key in ((column - 1, row - 1), (column, row - 1), (column - 1, row), (column, row)):
            if key in tiles:
                flow.add(node, tile_in[key], 1, 0)
    through: dict[tuple[int, int], int] = {}
    for key in ordered_tiles:
        tile = tiles[key]
        diagonal = min(capacity(gap) for gap in tile.diagonals)
        through[key] = flow.add(tile_in[key], tile_in[key] + 1, diagonal, 0)
        column, row = key
        # W, N, E, S: the neighbouring tile across each side, or the outside of the field.
        for side, neighbour in zip(tile.sides, ((column - 1, row), (column, row - 1), (column + 1, row),
                                                (column, row + 1))):
            target = tile_in.get(neighbour, sink)
            flow.add(tile_in[key] + 1, target, capacity(side), 1)
    flow.run(source, sink)
    surface = tuple(pad for pad in ordered_balls if flow.used(source, ball_node[pad]))
    vias = tuple(pad for pad in ordered_balls if pad not in set(surface))
    load = {key: flow.edge_flow(tile_in[key], through[key]) for key in ordered_tiles}
    return ArrayEscape(component, layer, surface, vias, MappingProxyType(load))


class _MinCostFlow:
    """Integer min-cost max-flow by successive shortest paths (Dijkstra with potentials).

    Deterministic: edges are kept in insertion order and ties go to the lower node.
    """

    def __init__(self, nodes: int) -> None:
        self.graph: list[list[list[int]]] = [[] for _ in range(nodes)]

    def add(self, start: int, end: int, capacity: int, cost: int) -> int:
        """Add an edge; returns its index in ``start``'s list."""
        self.graph[start].append([end, capacity, cost, len(self.graph[end]), capacity])
        self.graph[end].append([start, 0, -cost, len(self.graph[start]) - 1, 0])
        return len(self.graph[start]) - 1

    def edge_flow(self, start: int, index: int) -> int:
        edge = self.graph[start][index]
        return edge[4] - edge[1]

    def used(self, start: int, end: int) -> bool:
        return any(edge[0] == end and edge[4] and edge[1] == 0 for edge in self.graph[start])

    def run(self, source: int, sink: int) -> tuple[int, int]:
        from heapq import heappop, heappush
        size = len(self.graph)
        potential = [0] * size
        total_flow = total_cost = 0
        while True:
            distance = [None] * size
            parent: list[tuple[int, int] | None] = [None] * size
            distance[source] = 0
            heap = [(0, source)]
            while heap:
                cost, node = heappop(heap)
                if cost > distance[node]:
                    continue
                for index, (end, residual, edge_cost, _reverse, _capacity) in enumerate(self.graph[node]):
                    if residual <= 0:
                        continue
                    candidate = cost + edge_cost + potential[node] - potential[end]
                    if distance[end] is None or candidate < distance[end]:
                        distance[end] = candidate
                        parent[end] = (node, index)
                        heappush(heap, (candidate, end))
            if distance[sink] is None:
                return total_flow, total_cost
            for node in range(size):
                if distance[node] is not None:
                    potential[node] += distance[node]
            push, node = None, sink
            while node != source:
                previous, index = parent[node]
                residual = self.graph[previous][index][1]
                push = residual if push is None else min(push, residual)
                node = previous
            node = sink
            while node != source:
                previous, index = parent[node]
                edge = self.graph[previous][index]
                edge[1] -= push
                self.graph[node][edge[3]][1] += push
                total_cost += push * edge[2]
                node = previous
            total_flow += push


# --------------------------------------------------------------------------- geometry

def _hull(points: list[Point]) -> tuple[Point, ...]:
    """Strict convex hull, counter-clockwise in board coordinates."""
    unique = sorted(set(points), key=lambda p: (p.x_nm, p.y_nm))
    if len(unique) <= 2:
        return tuple(unique)

    def half(sequence):
        chain: list[Point] = []
        for point in sequence:
            while len(chain) >= 2 and orientation(chain[-2], chain[-1], point) <= 0:
                chain.pop()
            chain.append(point)
        return chain[:-1]

    return tuple(half(unique) + half(reversed(unique)))


def _on_boundary(point: Point, hull: tuple[Point, ...]) -> bool:
    if len(hull) <= 2:
        return True
    return any(point_on_segment(point, hull[index], hull[(index + 1) % len(hull)]) for index in range(len(hull)))


def _centre(points: list[Point]) -> tuple[float, float]:
    """Mean pin position: inside the pins' convex hull."""
    return (sum(p.x_nm for p in points) / len(points), sum(p.y_nm for p in points) / len(points))


def _angle(point: Point, centre: tuple[float, float], cut: float, sense: float = 1.0) -> float:
    """Angle of ``point`` around ``centre`` from ``cut``, in [0, 2π), increasing in ``sense``."""
    angle = sense * (atan2(point.y_nm - centre[1], point.x_nm - centre[0]) - cut)
    # Rounded so pins in one column of a rotated part keep a stable order.
    return round(angle % (2 * pi), 9) % round(2 * pi, 9)


def _layer_rank(layer: CopperLayer) -> int:
    if layer is CopperLayer.FRONT:
        return 0
    if layer is CopperLayer.BACK:
        return 99
    return int(layer.value[2:-3])


def _pad_text(pad: PadReference) -> str:
    return f"{pad.component}.{pad.pad}"
