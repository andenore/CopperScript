"""Bundle-aware ordering for critical differential groups.

Critical groups are routed one at a time and accepted copper is immutable, so
the order matters when several pairs share one breakout. A *bundle* is two or
more differential (or CAN) groups of the same kind and priority whose members
each connect the same two components, for example the lanes and clock of a
camera link between a QFN and a connector. Inside a bundle the groups are
routed in physical order along the terminal row, outermost first, instead of
name order. Groups outside bundles keep the priority/kind/name order.

This module only plans the order and holds the per-bundle report. The critical
router owns routing, validation and the bounded repair (``critical.py``).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Mapping, Sequence

from .physical import NetRoutingRule, PhysicalBoard, Point, RouteKind


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
class CriticalBundle:
    """Routing order and repair record of one bundle of critical groups."""

    components: tuple[str, str]
    kind: str
    priority: int
    order: tuple[tuple[str, ...], ...]
    name_order: tuple[tuple[str, ...], ...]
    repair_limit: int
    repairs: tuple[BundleRepair, ...] = ()

    @property
    def repairs_attempted(self) -> int:
        return len(self.repairs)

    @property
    def repairs_accepted(self) -> int:
        return sum(item.accepted for item in self.repairs)


def bundle_document(bundle: CriticalBundle) -> dict[str, object]:
    """JSON-ready form of one bundle for the critical report."""
    return {
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


def plan_bundles(
    board: PhysicalBoard,
    jobs: Sequence[tuple[NetRoutingRule, tuple[str, ...]]],
    rules: Mapping[str, NetRoutingRule],
    repair_limit: int,
) -> tuple[CriticalBundle, ...]:
    """Find the bundles among ``jobs`` (in default order) and their physical order.

    A group joins a bundle when its rules are symmetric pair rules and each
    member net has exactly two pads, one on each of the same two placed
    components. Groups that share components, kind and priority form a bundle
    when there are at least two of them.
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
        bundles.append(CriticalBundle(
            (first, second), kind, priority,
            _physical_order(entries, (first, second)),
            tuple(group for group, _ in entries),
            repair_limit,
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
