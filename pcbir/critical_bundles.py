"""Bundle-aware ordering for critical differential groups.

Critical groups are routed one at a time and accepted copper is immutable, so
the order matters when several pairs share one breakout. A *bundle* is two or
more differential (or CAN) groups of the same kind and priority whose members
each connect the same two components, for example the lanes and clock of a
camera link between a QFN and a connector. Inside a bundle the groups are
routed in physical order along the terminal row, outermost first, instead of
name order. Groups outside bundles keep the priority/kind/name order.

When the pair order at the two ends of a bundle is inverted, some pairs must
cross others (plan R5). The fewest pairs move off the surface, each with one
planned paired layer swap; they are routed first, then the surface pairs in
physical order (``_plan_crossings``).

This module only plans the order and the crossings and holds the per-bundle
report. The critical router owns routing, validation and the bounded repair
(``critical.py``).
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from functools import cmp_to_key
from math import hypot
from typing import Mapping, Sequence

from .physical import (BoardSide, CopperLayer, NetRoutingRule, PhysicalBoard, Point, RouteKind,
                       Via)
from .routing_layers import routing_layers, signal_layer_preferences
from .routing_vias import physical_via_span


# Default bound on rip-up repairs per bundle (``route_critical_nets`` accepts
# ``bundle_repair_limit``). Each attempt routes at most two groups.
BUNDLE_REPAIR_LIMIT = 4

_PAIR_KINDS = frozenset({RouteKind.DIFFERENTIAL, RouteKind.CAN_BUS})


@dataclass(frozen=True, slots=True)
class BundleRepair:
    """One bounded rip-up attempt inside a bundle."""

    failed: tuple[str, ...]
    ripped_up: tuple[str, ...]
    accepted: bool
    reason: str


@dataclass(frozen=True, slots=True)
class CrossingTransition:
    """One routed paired transition of a planned crossing."""

    component: str  # the terminal component the transition is next to
    vias: tuple[tuple[str, Point], ...]  # (net, centre) of each member's via
    return_vias: tuple[Point, ...]
    # "return_vias", "shared_reference" or "not_required".
    reference: str
    shared_reference_layer: CopperLayer | None = None


@dataclass(frozen=True, slots=True)
class PlannedCrossing:
    """A bundle pair that must cross others, with its planned paired layer swap.

    ``status`` is "planned" until routed, then "routed" or "failed"; it is
    "impossible" when no paired layer swap is allowed (``layer`` is None and
    ``reason`` says why).
    """

    group: tuple[str, ...]
    crosses: tuple[tuple[str, ...], ...]
    surface: CopperLayer
    layer: CopperLayer | None
    reference_planes: tuple[CopperLayer, ...] = ()
    status: str = "planned"
    reason: str = ""
    transitions: tuple[CrossingTransition, ...] = ()


@dataclass(frozen=True, slots=True)
class CriticalBundle:
    """Routing order and repair record of one bundle of critical groups."""

    components: tuple[str, str]
    kind: str
    priority: int
    order: tuple[tuple[str, ...], ...]
    name_order: tuple[tuple[str, ...], ...]
    repair_limit: int
    repairs: tuple[BundleRepair, ...] = ()
    # Pairs that must cross others, moved off the surface (plan R5).
    crossings: tuple[PlannedCrossing, ...] = ()

    @property
    def repairs_attempted(self) -> int:
        return len(self.repairs)

    @property
    def repairs_accepted(self) -> int:
        return sum(item.accepted for item in self.repairs)


def bundle_document(bundle: CriticalBundle) -> dict[str, object]:
    """JSON-ready form of one bundle for the critical report.

    ``crossings`` is present only for a bundle with planned crossings, so the
    reports of other bundles are unchanged.
    """
    document: dict[str, object] = {
        "components": list(bundle.components),
        "kind": bundle.kind,
        "priority": bundle.priority,
        "order": [list(group) for group in bundle.order],
        "name_order": [list(group) for group in bundle.name_order],
        "repair_limit": bundle.repair_limit,
        "repairs_attempted": bundle.repairs_attempted,
        "repairs_accepted": bundle.repairs_accepted,
        "repairs": [
            {"failed": list(item.failed), "ripped_up": list(item.ripped_up),
             "accepted": item.accepted, "reason": item.reason}
            for item in bundle.repairs
        ],
    }
    if bundle.crossings:
        document["crossings"] = [_crossing_document(item) for item in bundle.crossings]
    return document


def _crossing_document(crossing: PlannedCrossing) -> dict[str, object]:
    return {
        "group": list(crossing.group),
        "crosses": [list(group) for group in crossing.crosses],
        "surface": crossing.surface.value,
        "layer": crossing.layer.value if crossing.layer is not None else None,
        "reference_planes": [layer.value for layer in crossing.reference_planes],
        "status": crossing.status,
        "reason": crossing.reason,
        "transitions": [
            {"component": item.component,
             "vias": {net: [point.x_nm, point.y_nm] for net, point in item.vias},
             "reference": item.reference,
             "return_vias": [[point.x_nm, point.y_nm] for point in item.return_vias],
             "shared_reference_layer": (item.shared_reference_layer.value
                                        if item.shared_reference_layer is not None else None)}
            for item in crossing.transitions
        ],
    }


def crossing_line(bundle: CriticalBundle, crossing: PlannedCrossing) -> str:
    """One console line for a planned crossing (the critical preflight)."""
    def sites(points: Sequence[Point]) -> str:
        return "/".join(f"({point.x_nm / 1e6:.3f}, {point.y_nm / 1e6:.3f})" for point in points) + " mm"

    head = (f"bundle {'-'.join(bundle.components)} crossing {'/'.join(crossing.group)} under "
            f"{', '.join('/'.join(group) for group in crossing.crosses)}: {crossing.status}")
    if crossing.layer is None:
        return f"{head} ({crossing.reason})"
    planes = ",".join(layer.value for layer in crossing.reference_planes) or "none"
    line = (f"{head}, {crossing.surface.value} -> {crossing.layer.value} -> {crossing.surface.value}"
            f" (reference planes {planes})")
    for item in crossing.transitions:
        decision = (f"return vias {sites(item.return_vias)}" if item.reference == "return_vias" else
                    f"shared reference {item.shared_reference_layer.value}"
                    if item.shared_reference_layer is not None else "no return vias required")
        line += f"; {item.component} vias {sites([point for _, point in item.vias])}, {decision}"
    if crossing.status == "failed":
        line += f"; {crossing.reason}"
    return line


def plan_bundles(
    board: PhysicalBoard,
    jobs: Sequence[tuple[NetRoutingRule, tuple[str, ...]]],
    rules: Mapping[str, NetRoutingRule],
    repair_limit: int,
    *, plan_crossings: bool = True,
) -> tuple[CriticalBundle, ...]:
    """Find the bundles among ``jobs`` (in default order) and their physical order.

    A group joins a bundle when its rules are symmetric pair rules and each
    member net has exactly two pads, one on each of the same two placed
    components. Groups that share components, kind and priority form a bundle
    when there are at least two of them. With ``plan_crossings``, a bundle
    whose pairs must cross gets its planned crossings, and its moving pairs
    come first in its order (plan R5).
    """
    nets = {item.name: item for item in board.nets}
    placements = {item.reference: item for item in board.placements}
    members: dict[tuple[str, int, str, str], list[tuple[tuple[str, ...], dict[str, Point]]]] = {}
    for rule, group in jobs:
        if rule.kind not in _PAIR_KINDS or len(group) != 2:
            continue
        partner = rules.get(rule.differential_partner or "")
        if partner is None or partner.differential_partner != rule.net or partner.kind is not rule.kind:
            continue
        lands = _group_lands(board, nets, placements, group)
        if lands is None:
            continue
        first, second = sorted(lands)
        key = (rule.kind.value, rule.priority, first, second)
        members.setdefault(key, []).append((group, lands))
    bundles = []
    for (kind, priority, first, second), entries in members.items():
        if len(entries) < 2:
            continue
        order = _physical_order(entries, (first, second))
        crossings = (_plan_crossings(board, entries, (first, second), order, rules)
                     if plan_crossings else ())
        if crossings:
            # Moving pairs first, so the surface pairs route around their
            # transition sites; each part keeps the physical order.
            moving = {item.group for item in crossings}
            order = (*(group for group in order if group in moving),
                     *(group for group in order if group not in moving))
        bundles.append(CriticalBundle(
            (first, second), kind, priority, order,
            tuple(group for group, _ in entries),
            repair_limit, crossings=crossings,
        ))
    # Report bundles in the order their first group would have been routed.
    position = {group: index for index, (_, group) in enumerate(jobs)}
    return tuple(sorted(bundles, key=lambda item: min(position[group] for group in item.order)))


def bundle_job_order(
    jobs: Sequence[tuple[NetRoutingRule, tuple[str, ...]]],
    bundles: Sequence[CriticalBundle],
) -> list[tuple[NetRoutingRule, tuple[str, ...]]]:
    """Reorder ``jobs`` so each bundle's groups follow the bundle order.

    A bundle keeps the slots its groups held in the default order, so groups
    outside bundles, and the bundle's place among them, are unchanged.
    """
    by_group = {group: (rule, group) for rule, group in jobs}
    replacement: dict[tuple[str, ...], tuple[str, ...]] = {}
    for bundle in bundles:
        members = set(bundle.order)
        slots = [group for _, group in jobs if group in members]
        replacement.update(zip(slots, bundle.order))
    return [by_group[replacement.get(group, group)] for _, group in jobs]


def _group_lands(
    board: PhysicalBoard, nets: Mapping[str, object], placements: Mapping[str, object],
    group: tuple[str, ...],
) -> dict[str, Point] | None:
    """Mean land position of a group per component, when it joins exactly two."""
    from .placement import PlacementAlgorithmError, transformed_pad_position

    positions: dict[str, list[Point]] = {}
    components: set[frozenset[str]] = set()
    for name in group:
        net = nets.get(name)
        if net is None or len(net.pads) != 2:
            return None
        references = frozenset(pad.component for pad in net.pads)
        if len(references) != 2 or not references <= set(placements):
            return None
        components.add(references)
        for pad in net.pads:
            try:
                point = transformed_pad_position(board, placements[pad.component], pad.pad)
            except PlacementAlgorithmError:
                return None
            positions.setdefault(pad.component, []).append(point)
    if len(components) != 1:
        return None
    return {reference: Point(sum(p.x_nm for p in points) // len(points),
                             sum(p.y_nm for p in points) // len(points))
            for reference, points in positions.items()}


def _physical_order(
    entries: list[tuple[tuple[str, ...], dict[str, Point]]], components: tuple[str, str],
) -> tuple[tuple[str, ...], ...]:
    """Outermost groups first, by their rank along each component's terminal row.

    Each component's row axis is x or y, whichever spreads the bundle's land
    centres more (x on a tie). A group's depth is its distance in ranks from
    the nearer end of the row, summed over both components, so a bundle whose
    order is consistent at both ends is routed from both edges inward. Ties go
    to the lower rank at the first component, then to the net names.
    """
    count = len(entries)
    ranks: list[dict[tuple[str, ...], int]] = []
    for reference in components:
        points = [lands[reference] for _, lands in entries]
        spread_x = max(p.x_nm for p in points) - min(p.x_nm for p in points)
        spread_y = max(p.y_nm for p in points) - min(p.y_nm for p in points)
        horizontal = spread_x >= spread_y
        ordered = sorted(entries, key=lambda entry: (
            entry[1][reference].x_nm if horizontal else entry[1][reference].y_nm,
            entry[1][reference].y_nm if horizontal else entry[1][reference].x_nm,
            entry[0]))
        ranks.append({group: rank for rank, (group, _) in enumerate(ordered)})

    def depth(group: tuple[str, ...]) -> int:
        return sum(min(rank[group], count - 1 - rank[group]) for rank in ranks)

    return tuple(sorted((group for group, _ in entries),
                        key=lambda group: (depth(group), ranks[0][group], group)))


def crossing_outcome(
    board: PhysicalBoard, components: tuple[str, str], crossing: PlannedCrossing,
    rules: Mapping[str, NetRoutingRule], connected: bool, reason: str, vias: Sequence[Via],
) -> PlannedCrossing:
    """A planned crossing's record after routing, from its group's accepted vias.

    Each transition is named by the nearer terminal component of the pair. A
    transition without return vias uses the declared shared reference when
    the pair requires return vias (the candidate gates only accept that when
    the reference covers it, CS-160), else none are required.
    """
    if crossing.layer is None:
        return crossing
    if not connected:
        return replace(crossing, status="failed", reason=reason)
    nets = {item.name: item for item in board.nets}
    placements = {item.reference: item for item in board.placements}
    lands = _group_lands(board, nets, placements, crossing.group)
    assert lands is not None
    sites: dict[str, list[Via]] = {}
    for via in sorted((via for via in vias if via.net in crossing.group),
                      key=lambda via: (via.net, via.position.x_nm, via.position.y_nm)):
        nearest = min(components, key=lambda reference: (_distance(via.position, lands[reference]), reference))
        sites.setdefault(nearest, []).append(via)
    centres = {reference: Point(sum(v.position.x_nm for v in items) // len(items),
                                sum(v.position.y_nm for v in items) // len(items))
               for reference, items in sites.items()}
    returns: dict[str, list[Point]] = {}
    for via in vias:
        if via.net not in crossing.group and centres:
            nearest = min(centres, key=lambda reference: (_distance(via.position, centres[reference]), reference))
            returns.setdefault(nearest, []).append(via.position)
    members = [rules[net] for net in crossing.group]
    required = any(rule.require_return_vias for rule in members)
    transitions = []
    for reference in (item for item in components if item in sites):
        points = tuple(sorted(returns.get(reference, ()), key=lambda point: (point.x_nm, point.y_nm)))
        shared = members[0].shared_reference_layer if required and not points else None
        transitions.append(CrossingTransition(
            reference, tuple((via.net, via.position) for via in sites[reference]), points,
            "return_vias" if points else "shared_reference" if shared is not None else "not_required",
            shared))
    return replace(crossing, status="routed", reason="", transitions=tuple(transitions))


def _distance(first: Point, second: Point) -> float:
    return hypot(first.x_nm - second.x_nm, first.y_nm - second.y_nm)


def _plan_crossings(
    board: PhysicalBoard, entries: list[tuple[tuple[str, ...], dict[str, Point]]],
    components: tuple[str, str], order: tuple[tuple[str, ...], ...],
    rules: Mapping[str, NetRoutingRule],
) -> tuple[PlannedCrossing, ...]:
    """The planned crossings of one bundle, in bundle order; empty without any.

    Two pairs must cross when their ranks at the two ends (``_crossing_ranks``)
    are in opposite order. The surface keeps the pairs ``_surface_pairs``
    chooses; every other pair gets one planned layer (``_crossing_layers``),
    never the layer of another moving pair it crosses, preferably the layer of
    a moving pair of its own ``layer_group``. A pair with no layer left is an
    impossible crossing with the reason.
    """
    placements = {item.reference: item for item in board.placements}
    sides = {placements[reference].side for reference in components}
    ranks = _crossing_ranks(board, entries, components) if len(sides) == 1 else None
    if ranks is None:
        return ()
    first, aligned = ranks
    groups = [group for group, _ in entries]
    crossed = {group: {other for other in groups
                       if (first[group] - first[other]) * (aligned[group] - aligned[other]) < 0}
               for group in groups}
    if not any(crossed.values()):
        return ()
    surface = CopperLayer.FRONT if next(iter(sides)) is BoardSide.FRONT else CopperLayer.BACK
    options = {group: _crossing_layers(board, group, rules, surface) for group in groups}
    position = {group: index for index, group in enumerate(order)}
    kept = _surface_pairs(sorted(groups, key=lambda group: first[group]), aligned,
                          {group: bool(options[group][0]) for group in groups}, position)
    assigned: dict[tuple[str, ...], CopperLayer] = {}
    crossings: list[PlannedCrossing] = []
    for group in order:
        if group in kept:
            continue
        crosses = tuple(other for other in order if other in crossed[group])
        layers, reason = options[group]
        taken = {assigned[other] for other in crossed[group] if other in assigned}
        free = [layer for layer in layers if layer not in taken]
        if not free:
            if layers:
                reason = (f"it crosses moving pairs on {', '.join(sorted(layer.value for layer in taken))}"
                          " and no other allowed layer is left")
            crossings.append(PlannedCrossing(group, crosses, surface, None, status="impossible", reason=reason))
            continue
        names = {rules[net].layer_group for net in group} - {None}
        shared = [layer for other, layer in assigned.items()
                  if layer in free and names & {rules[net].layer_group for net in other}]
        layer = shared[0] if shared else free[0]
        assigned[group] = layer
        crossings.append(PlannedCrossing(group, crosses, surface, layer, _reference_planes(board, layer)))
    return tuple(crossings)


def _crossing_ranks(
    board: PhysicalBoard, entries: list[tuple[tuple[str, ...], dict[str, Point]]],
    components: tuple[str, str],
) -> tuple[dict[tuple[str, ...], int], dict[tuple[str, ...], int]] | None:
    """Each group's rank at both ends, equal for groups that need not cross.

    At each component the pair centres are ordered by angle around the
    component's courtyard centre, in one rotational sense for both components,
    starting from the direction that points away from the other component.
    Pairs that leave both rows side by side then appear in reversed order at
    the two ends, so the second component's ranks are reversed. Comparisons
    are exact integer cross products. ``None`` when the reading is ambiguous:
    coincident component centres, a pair centre on its component's centre, or
    two pair centres at one angle.
    """
    from .placement import _placement_polygon

    placements = {item.reference: item for item in board.placements}
    centres: dict[str, tuple[int, int]] = {}
    for reference in components:
        polygon = _placement_polygon(board, placements[reference])
        xs, ys = [point.x_nm for point in polygon], [point.y_nm for point in polygon]
        centres[reference] = ((min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2)
    ranks: list[dict[tuple[str, ...], int]] = []
    for reference, other in (components, components[::-1]):
        cx, cy = centres[reference]
        away = (cx - centres[other][0], cy - centres[other][1])
        vectors = {group: (lands[reference].x_nm - cx, lands[reference].y_nm - cy)
                   for group, lands in entries}
        if away == (0, 0) or (0, 0) in vectors.values():
            return None

        def half(vector: tuple[int, int]) -> int:
            cross = away[0] * vector[1] - away[1] * vector[0]
            dot = away[0] * vector[0] + away[1] * vector[1]
            return 0 if cross > 0 or (cross == 0 and dot > 0) else 1

        def compare(first: tuple[str, ...], second: tuple[str, ...]) -> int:
            a, b = vectors[first], vectors[second]
            if half(a) != half(b):
                return half(a) - half(b)
            cross = a[0] * b[1] - a[1] * b[0]
            return -1 if cross > 0 else 1 if cross < 0 else 0

        ordered = sorted(vectors, key=cmp_to_key(compare))
        if any(compare(a, b) == 0 for a, b in zip(ordered, ordered[1:])):
            return None
        ranks.append({group: rank for rank, group in enumerate(ordered)})
    last = len(entries) - 1
    return ranks[0], {group: last - rank for group, rank in ranks[1].items()}


def _surface_pairs(
    sequence: list[tuple[str, ...]], aligned: Mapping[tuple[str, ...], int],
    swappable: Mapping[tuple[str, ...], bool], position: Mapping[tuple[str, ...], int],
) -> frozenset[tuple[str, ...]]:
    """The groups that stay on the surface: no two of them cross.

    ``sequence`` is in rank order at the first component, so the kept groups
    have increasing ``aligned`` ranks. The set keeps the most groups that
    cannot swap layers, then the most groups (the fewest move), then the inner
    groups (later in the bundle order, so transitions sit at the bundle's
    edge), then the lowest ranks at the first component.
    """
    def preferred(a: tuple[tuple[int, ...], tuple[int, ...]],
                  b: tuple[tuple[int, ...], tuple[int, ...]]) -> bool:
        return a[0] > b[0] or (a[0] == b[0] and a[1] < b[1])

    best: list[tuple[tuple[int, ...], tuple[int, ...]]] = []
    for index, group in enumerate(sequence):
        own = (0 if swappable[group] else 1, 1, position[group])
        choice = (own, (index,))
        for previous in range(index):
            if aligned[sequence[previous]] < aligned[group]:
                score, chain = best[previous]
                candidate = (tuple(a + b for a, b in zip(score, own)), (*chain, index))
                if preferred(candidate, choice):
                    choice = candidate
        best.append(choice)
    chosen = best[0]
    for item in best[1:]:
        if preferred(item, chosen):
            chosen = item
    return frozenset(sequence[index] for index in chosen[1])


def _crossing_layers(
    board: PhysicalBoard, group: tuple[str, ...], rules: Mapping[str, NetRoutingRule],
    surface: CopperLayer,
) -> tuple[tuple[CopperLayer, ...], str]:
    """Candidate crossing layers of a pair, best first, or none and the reason.

    A layer both members allow (``routing_layers``), and every member of their
    ``layer_group`` where declared, other than the terminal surface and with a
    legal via span from it. Layers with a reference plane (L3) are used when
    any qualifies. A member whose ``max_vias`` is below 2 cannot swap.
    """
    members = [rules[net] for net in group]
    for rule in members:
        if rule.max_vias is not None and rule.max_vias < 2:
            return (), (f"max_vias {rule.max_vias} on {rule.net} allows no paired layer swap; "
                        "it needs 2 vias per member")
    allowed = set(routing_layers(board, members[0].net, members[0]))
    allowed &= set(routing_layers(board, members[1].net, members[1]))
    if surface not in allowed:
        return (), f"the terminal surface {surface.value} is not an allowed layer"
    names = sorted({rule.layer_group for rule in members if rule.layer_group is not None})
    for rule in board.net_routing_rules:
        if rule.layer_group in names:
            allowed &= set(routing_layers(board, rule.net, rule))
    layers = [layer for layer in allowed
              if layer is not surface and physical_via_span(board, surface, layer) is not None]
    if not layers:
        scope = f" of layer group {', '.join(names)}" if names else ""
        return (), (f"no allowed layer{scope} besides the terminal surface {surface.value} "
                    "with a legal via span")
    referenced = [layer for layer in layers if _reference_planes(board, layer)]
    ranks, _ = signal_layer_preferences(board)
    copper = board.stackup.copper_layers
    return tuple(sorted(referenced or layers, key=lambda layer: (ranks[layer], copper.index(layer)))), ""


def _reference_planes(board: PhysicalBoard, layer: CopperLayer) -> tuple[CopperLayer, ...]:
    """Adjacent copper layers with a declared ``copper_zone`` (plan L3)."""
    copper = board.stackup.copper_layers
    zoned = {item for zone in board.zones for item in zone.layers}
    index = copper.index(layer)
    return tuple(copper[i] for i in (index - 1, index + 1) if 0 <= i < len(copper) and copper[i] in zoned)
