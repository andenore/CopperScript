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

The model is advisory and never changes routing. Every number is computed
from integer pad geometry with deterministic tie-breaks.
"""
from __future__ import annotations

from bisect import bisect_right
from dataclasses import dataclass
from math import atan2, isqrt, pi

from .drc import placed_pad_shape
from .geometry import Bounds, bounds, orientation, point_on_segment, shape_distance_squared
from .physical import BoardSide, CopperLayer, PadKind, PadReference, PhysicalBoard, Point
from .placement import transformed_local_point
from .zone_geometry import distribution_zone_nets


@dataclass(frozen=True, slots=True)
class EscapePlanOptions:
    # Two components sharing this many signal nets are planned as a pair.
    cluster_pair_minimum_nets: int = 2
    # Pairs with more shared nets are reported without an order analysis.
    maximum_pair_nets: int = 256


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
                "overflow": self.overflow}


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
                     if (cluster := _cluster(board, placement, pad_nets, signal)) is not None)
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
             signal: set[str]) -> ClusterEscape | None:
    footprint = board.footprints[placement.footprint]
    pads = [pad for pad in footprint.pads if pad.kind not in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}]
    if not pads:
        return None
    surface = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
    placed = []
    for pad in pads:
        position = transformed_local_point(placement, pad.position)
        layers = (surface,) if pad.kind is PadKind.SMD else tuple(board.stackup.copper_layers)
        placed.append((pad, position, layers, placed_pad_shape(position, pad, placement)))
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
    return ClusterEscape(placement.reference, pins, bounds(pin.position for pin in pins), inner, channels)


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
    """The widest track and clearance among ``nets`` (each net's own rule, else the board default)."""
    rules = {rule.net: rule for rule in board.net_routing_rules}
    width = max(max(board.rules.minimum_track_width_nm,
                    (rules[net].width_nm if net in rules and rules[net].width_nm else
                     board.rules.default_track_width_nm)) for net in nets)
    clearance = max(max(board.rules.minimum_clearance_nm,
                        (rules[net].clearance_nm or 0) if net in rules else 0) for net in nets)
    return width, clearance


def _edge_gap(first, second) -> int:
    distance = isqrt(int(shape_distance_squared(first, second)))
    return max(0, distance - first.radius_nm - second.radius_nm)


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
                             tuple(net for net in nets if net not in kept))


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
